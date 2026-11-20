#!/usr/bin/env python3
"""Read-only native usage collectors. Python 3.9+, no third-party dependencies.

Each run reads changed native files into a private SQLite ledger, then publishes
cycle summaries. Run --help for the standalone interface. No agent is launched.
"""
import argparse
from contextlib import contextmanager
from datetime import datetime, timezone
import json
import math
import os
import re
from pathlib import Path
import sqlite3
import sys
import tempfile
import time

SOURCES = ('codex', 'claude', 'grok', 'agy', 'kiro', 'droid', 'opencode', 'codebuddy')
UTC = timezone.utc
TOKEN_KEYS = ('input', 'output', 'cache_read', 'cache_write', 'cache_write_long')


def stamp(value):
    try:
        if isinstance(value, (int, float)):
            return float(value) / (1000 if value > 1e11 else 1)
        parsed = datetime.fromisoformat(str(value).replace('Z', '+00:00'))
        return parsed.timestamp() if parsed.tzinfo else None
    except (ValueError, TypeError, OverflowError):
        return None


def iso(value):
    return datetime.fromtimestamp(value, UTC).isoformat()


def number(value):
    return isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(value) and value >= 0


def read_json(path, default=None):
    try:
        return json.loads(Path(path).read_text(encoding='utf-8'))
    except FileNotFoundError:
        return default


def save_json(path, value):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    fd, temporary = tempfile.mkstemp(dir=path.parent, prefix='.usage-')
    try:
        with os.fdopen(fd, 'w', encoding='utf-8') as stream:
            json.dump(value, stream, ensure_ascii=False, allow_nan=False, separators=(',', ':'))
        os.replace(temporary, path)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


@contextmanager
def lock(path):
    with open(path, 'a+b') as stream:
        if os.name == 'nt':
            import msvcrt
            stream.seek(0); stream.write(b'0'); stream.flush(); stream.seek(0)
            msvcrt.locking(stream.fileno(), msvcrt.LK_NBLCK, 1)
        else:
            import fcntl
            fcntl.flock(stream, fcntl.LOCK_EX | fcntl.LOCK_NB)
        yield


def event(identity, when, model, usage, *, included=False, native_cost=None, partial=False):
    values = {key: usage.get(key, 0) for key in TOKEN_KEYS}
    valid = all(number(v) for v in values.values())
    if included and valid:
        values['input'] -= values['cache_read'] + values['cache_write'] + values['cache_write_long']
        valid = values['input'] >= 0
    return {'id': str(identity), 'at': stamp(when), 'model': model or '', 'tokens': values,
            'partial': partial or not valid, 'native_cost': native_cost if number(native_cost) else None}


def incomplete(state, when=None):
    state['partial'] = True
    at = stamp(when)
    if at is None:
        state['partial_unknown'] = True
    else:
        state['partial_at'] = max(state.get('partial_at', at), at)


def agy_model_notice(text):
    if not isinstance(text,str) or not text.startswith('<USER_REQUEST>\n') or '\n</USER_REQUEST>' not in text:
        return None
    tail = text.rsplit('\n</USER_REQUEST>',1)[1]
    found = None
    while tail.strip():
        tail = tail.lstrip()
        tag = next((t for t in ('ADDITIONAL_METADATA','USER_SETTINGS_CHANGE') if tail.startswith('<'+t+'>')), None)
        if not tag or '</'+tag+'>' not in tail:
            return None
        body, tail = tail[len(tag)+2:].split('</'+tag+'>',1)
        if tag == 'USER_SETTINGS_CHANGE':
            match = re.fullmatch(r'The user changed setting `Model Selection` from .*? to (.*?)\. No need to comment on this change.*',body.strip(),re.S)
            if match and match[1] != 'None':
                found = match[1]
    if found:
        gemini = re.fullmatch(r'Gemini (\d+) (Carbon|Argon|Hydrogen)(?: \((Low|Medium|High|Extra)\))?',found)
        if gemini:
            return 'gemini-' + '-'.join(x.lower() for x in gemini.groups() if x)
    return found


def droid_record(raw):
    when, _, entry = raw.partition('] INFO: ')
    kind, _, body = entry.partition(' | Context: ')
    if not when.startswith('[') or kind not in ('[Agent] Streaming result', '[Model-Router] Model transition'):
        return []
    when = when[1:]
    # Only retain the scalar fields needed for metering and pool attribution.
    row = json.loads(body); tags = row.get('tags') or {}
    sid, model = tags.get('sessionId'), tags.get('modelId')
    process = row.get('factory.process.instance_id')
    if kind == '[Model-Router] Model transition':
        sid = row.get('sessionId') or sid
        at = stamp(when)
        if row.get('slot') != 'main' or not sid or not process or at is None:
            return []
        # Factory offers Kimi K3 in Droid Core. A normal model selection does
        # not establish the billing pool; the 402 recovery switch does.
        core_model = None
        if (row.get('source') == 'detector_recovery' and row.get('reason') == 'overage_reactive_402'
                and row.get('modelId') == 'kimi-k3'):
            core_model = row['modelId']
        return [{'kind':'droid_route', 'process':process, 'sid':sid, 'at':at, 'core_model':core_model}]
    identity = row.get('upstreamResponseId') or row.get('upstreamRequestId') or f"{sid}:{row.get('factory.process.instance_id')}:{when}"
    written = row.get('cachedTokensWritten',0)
    ttl = row.get('cacheWriteTtl')
    item = event(identity,when,model,{'input':row.get('inputTokens'), 'output':row.get('outputTokens'),
        'cache_read':row.get('cacheReadInputTokens',0), 'cache_write':written if ttl != '1h' else 0,
        'cache_write_long':written if ttl == '1h' else 0})
    item.update(sid=sid, process=process, byok=tags.get('isByok') in (True,'true'), cache_ttl_unknown=bool(written and ttl not in ('5m','1h')),
                pool=row.get('usagePool'))
    return [item]


def parse_usage_record(source, row, state, path, offset):
    """Return only scalar metering fields; never store conversation text."""
    if not isinstance(row, dict):
        return []
    kind = row.get('type')
    if source == 'codex':
        p = row.get('payload') or {}
        if kind == 'compacted' and p.get('compaction_response_id'):
            return [{'kind':'excluded', 'id':p['compaction_response_id'], 'at':stamp(row.get('timestamp'))}]
        if kind == 'session_meta':
            state['sid'] = p.get('id', path.stem)
            state['fork'] = bool(p.get('forked_from_id'))
        if kind == 'turn_context':
            state['model'] = p.get('model') or ''
        if kind == 'token_usage_record':
            state['per_request'] = True
            u = p.get('usage') or {}
            ident = p.get('response_id')
            if not ident:
                return []
        elif kind == 'event_msg' and p.get('type') == 'token_count' and not state.get('per_request'):
            info = p.get('info') or {}
            total = info.get('total_token_usage')
            if not isinstance(total, dict):
                return []
            previous = state.get('total')
            state['total'] = total
            # Legacy counters lack a request identity. Baseline conservatively;
            # never assign an inherited or pre-period total to the current cycle.
            if not previous:
                last = info.get('last_token_usage') or {}
                if all(total.get(k, 0) == last.get(k, 0) for k in ('input_tokens','cached_input_tokens','output_tokens')):
                    previous = {}
                else:
                    incomplete(state, row.get('timestamp'))
                    return []
            if any(total.get(k, 0) < v for k, v in previous.items() if number(v)):
                incomplete(state, row.get('timestamp'))
                return []
            u = {k: v - previous.get(k, 0) for k, v in total.items() if number(v)}
            if not any(u.values()):
                return []
            ident = f"{state.get('sid',path.stem)}:{row.get('timestamp')}:{total.get('total_tokens')}"
        else:
            return []
        return [{**event(ident, row.get('timestamp'), state.get('model'),
                      {'input': u.get('input_tokens'), 'output': u.get('output_tokens'),
                       'cache_read': u.get('cached_input_tokens', 0),
                       'cache_write': u.get('cache_write_input_tokens', 0)}, included=True,
                      partial=bool(u.get('cache_write_input_tokens'))),
                 'sid': p.get('thread_id') or state.get('sid', path.stem)}]
    if source == 'claude' and kind == 'assistant':
        msg = row.get('message') or {}; u = msg.get('usage') or {}
        if not u or not msg.get('id'):
            return []
        cache = u.get('cache_creation') or {}
        short = cache.get('ephemeral_5m_input_tokens', u.get('cache_creation_input_tokens', 0))
        long = cache.get('ephemeral_1h_input_tokens', 0)
        return [event(msg['id'], row.get('timestamp'), msg.get('model'),
                      {'input': u.get('input_tokens'), 'output': u.get('output_tokens'),
                       'cache_read': u.get('cache_read_input_tokens', 0),
                       'cache_write': short, 'cache_write_long': long})]
    if source == 'codebuddy':
        p = row.get('providerData') or {}; u = p.get('rawUsage') or {}
        ident = p.get('messageId')
        if not ident or not u:
            return []
        ident = f"{ident}:{p.get('stepSeq', '')}"
        return [event(ident, row.get('timestamp'), p.get('model'),
                      {'input': u.get('prompt_tokens'), 'output': u.get('completion_tokens'),
                       'cache_read': (u.get('prompt_tokens_details') or {}).get('cached_tokens', u.get('cached_tokens', 0)),
                       'cache_write': u.get('prompt_cache_write_tokens', 0)}, included=True,
                      partial=bool(u.get('prompt_cache_write_tokens')))]
    if source == 'agy' and kind == 'USER_INPUT' and row.get('status') == 'DONE':
        model = agy_model_notice(row.get('content'))
        if model:
            state['model'] = model
    if source == 'agy' and kind == 'PLANNER_RESPONSE':
        if row.get('status') != 'DONE' or 'input_tokens' not in row:
            return []
        sid = path.parents[2].name
        return [{**event(f"{sid}:{row.get('step_index',offset)}", row.get('created_at'), row.get('model') or state.get('model'),
                      {'input': row.get('input_tokens'), 'output': row.get('output_tokens'),
                       'cache_read': row.get('cache_read_tokens', 0)}), 'sid':sid}]
    return []


def parse_record(source, row, state, path, offset):
    items = parse_usage_record(source, row, state, path, offset)
    if not isinstance(row, dict):
        return items
    if source == 'codex' and row.get('type') == 'token_usage_record' and items:
        items[0]['meter'] = 'request'
        state['pending_request'] = items[0]
    if source == 'codex' and row.get('type') == 'event_msg':
        payload = row.get('payload') or {}
        limits = payload.get('rate_limits') or {}
        if payload.get('type') == 'token_count':
            # The quota and cumulative-counter report can arrive after the
            # response. Use its observation time for a matched request so a
            # first-percentage checkpoint has exactly the same numerator.
            pending = state.get('pending_request')
            info = payload.get('info') or {}
            last = info.get('last_token_usage') or {}
            if last or info.get('total_token_usage'):
                items.append({'kind':'counter_mode', 'id':state.get('sid',path.stem)})
            if pending:
                t = pending['tokens']
                if (last.get('input_tokens'),last.get('cached_input_tokens',0),last.get('output_tokens')) == (t['input']+t['cache_read'],t['cache_read'],t['output']):
                    pending = state.pop('pending_request')
                    items.append({**pending, 'at':stamp(row.get('timestamp')), 'reported':True})
            for name in ('primary', 'secondary'):
                bucket = limits.get(name) or {}
                if not number(bucket.get('used_percent')):
                    continue
                sid = state.get('sid', path.stem)
                items.append({'kind':'quota', 'id':f"{sid}:{row.get('timestamp')}:{limits.get('limit_id','codex')}:{name}",
                    'sid':sid, 'at':stamp(row.get('timestamp')), 'group':limits.get('limit_id') or 'codex',
                    'window':bucket.get('window_minutes'), 'end':stamp(bucket.get('resets_at')),
                    'used':bucket['used_percent'], 'plan':limits.get('plan_type')})
    for item in items:
        item.setdefault('sid', row.get('sessionId') or state.get('sid') or path.stem)
    return items


class Ownership:
    """Local move journals define execution intervals; file location does not."""
    def __init__(self, config):
        self.moves, self.files, self.gaps, self.identities = {}, {}, [], {}
        directory = config.get('transfer_dir')
        trash = config.get('trash_dir')
        if not directory:
            return
        for path in Path(directory).glob('*/operation.json'):
            op = read_json(path, {})
            if not op.get('moving') or op.get('phase') not in ('complete','moved','retiring','retired'):
                continue
            at = stamp(op.get('created_ms'))
            if at is None:
                continue
            outgoing = op.get('phase') in ('moved','retiring','retired') and not op.get('incoming_digest')
            members = (op.get('plan') or {}).get('group', {}).get('members', [])
            for member in members:
                key = (member.get('source'), member.get('sid'))
                if all(key):
                    self.moves.setdefault(key, []).append((at, not outgoing))
            if not outgoing:
                continue
            for source in {m.get('source') for m in members} & {'codex','claude','grok','codebuddy'}:
                manifest = Path(trash) / f"move-{op['id']}-{source}" / 'manifest.json' if trash else None
                saved = read_json(manifest, {}) if manifest else {}
                records = saved.get('files', [])
                if not records:
                    self.gaps.append((source, at))
                for record in records:
                    origin = Path(record.get('origin', ''))
                    if not (origin.suffix == '.jsonl' or source == 'grok' and origin.name == 'usage.json'):
                        continue
                    held = manifest.parent / 'files' / record['name']
                    file = held if record.get('in_trash') and held.exists() else origin
                    if file.is_file():
                        self.files.setdefault(source, []).append(file)
                        self.identities[(source,str(file))] = origin.stem
                    else:
                        self.gaps.append((source, at))
        for moves in self.moves.values():
            moves.sort()

    def contains(self, source, item):
        moves = self.moves.get((source, item.get('sid')), [])
        local = not moves or not moves[0][1]
        if item.get('at') is None:
            return local
        for at, incoming in moves:
            if at > item['at']:
                break
            local = incoming
        return local

    def incomplete_since(self, source, start):
        return any(s == source and (start is None or at >= start) for s, at in self.gaps)


class Ledger:
    def __init__(self, directory):
        # Preserve the old index, whose rows lack machine attribution and quotas.
        self.db = sqlite3.connect(directory / 'ledger-v2.sqlite')
        self.db.execute('pragma journal_mode=WAL')
        self.db.executescript('''
            create table if not exists events(source text, id text, at real, data text,
              primary key(source,id));
            create index if not exists events_time on events(source,at);
            create table if not exists files(source text,path text,device int,inode int,size int,
              mtime int,offset int,state text,primary key(source,path));
            create table if not exists quota_events(source text,id text,at real,data text,
              primary key(source,id));
            create index if not exists quota_time on quota_events(source,at);
            create table if not exists excluded_events(source text,id text,primary key(source,id));
            create table if not exists counter_sessions(source text,id text,primary key(source,id));
            create table if not exists parser_versions(source text primary key,version int);
            create table if not exists droid_routes(process text,sid text,at real,core_model text,
              primary key(process,sid,at));
        ''')
        self.bytes = 0
        self.changed = 0

    def put(self, source, item):
        if item.get('kind') == 'droid_route':
            self.db.execute('insert or replace into droid_routes values (?,?,?,?)',
                            (item['process'], item['sid'], item['at'], item['core_model']))
            return
        if item.get('kind') in ('excluded','counter_mode'):
            table = 'excluded_events' if item['kind'] == 'excluded' else 'counter_sessions'
            self.db.execute(f'insert or ignore into {table} values (?,?)', (source,item['id']))
            return
        table = 'quota_events' if item.get('kind') == 'quota' else 'events'
        self.db.execute(f'insert or replace into {table} values (?,?,?,?)',
                        (source, item['id'], item['at'], json.dumps(item)))

    def droid_pool(self, item):
        pool = item.get('pool')
        if pool or item['model'] != 'kimi-k3' or not item.get('process') or not item.get('sid'):
            return pool
        # Requests and routing transitions can straddle a log rotation.
        route = self.db.execute(
            'select core_model from droid_routes where process=? and sid=? and at<=? order by at desc limit 1',
            (item['process'], item['sid'], item['at'])).fetchone()
        return 'core' if route and route[0] == item['model'] else None

    def file(self, source, path, kind='jsonl', rebuild=False, sid=None):
        stat = path.stat()
        previous = self.db.execute('select device,inode,size,mtime,offset,state from files where source=? and path=?',
                                   (source, str(path))).fetchone()
        meta = (stat.st_dev, stat.st_ino, stat.st_size, stat.st_mtime_ns)
        if not rebuild and previous and tuple(previous[:4]) == meta:
            return
        state, offset = ({'sid':sid} if sid else {}), 0
        if kind in ('jsonl','droid_log') and not rebuild and previous and tuple(previous[:2]) == meta[:2] and stat.st_size > previous[2]:
            offset, state = previous[4], json.loads(previous[5])
        self.changed += 1
        if kind in ('jsonl','droid_log'):
            with path.open('rb') as stream:
                stream.seek(offset)
                while True:
                    raw = stream.readline()
                    self.bytes += len(raw)
                    if not raw or not raw.endswith(b'\n'):
                        break
                    try:
                        if kind == 'droid_log':
                            logged_at = stamp(raw.split(b']',1)[0][1:].decode('utf-8'))
                            if logged_at is not None:
                                state['first_at'] = min(state.get('first_at',logged_at),logged_at)
                                state['last_at'] = iso(logged_at)
                            for item in droid_record(raw.decode('utf-8')):
                                self.put(source,item)
                            offset = stream.tell()
                            continue
                        row = json.loads(raw)
                        if isinstance(row,dict) and stamp(row.get('timestamp')) is not None:
                            state['last_at'] = row['timestamp']
                        for item in parse_record(source, row, state, path, offset):
                            self.put(source, item)
                    except (ValueError, TypeError, AttributeError):
                        incomplete(state, state.get('last_at'))
                    offset = stream.tell()
        elif source == 'grok':
            value = read_json(path); self.bytes += stat.st_size
            for turn in value.get('turns', []):
                for model, u in (turn.get('modelUsage') or {}).items():
                    ident = f"{value.get('sessionId',path.parent.name)}:{turn.get('turnNumber')}:{model}"
                    self.put(source, {**event(ident, turn.get('endedAt'), model,
                        {'input': u.get('inputTokens'), 'output': u.get('outputTokens'),
                         'cache_read': u.get('cachedReadTokens', 0), 'cache_write': u.get('cacheCreationTokens', 0)},
                        included=True, native_cost=u['costUsdTicks'] / 1e10 if number(u.get('costUsdTicks')) else None,
                        partial=bool(turn.get('usageIsIncomplete') or turn.get('costIsPartial'))),
                        'sid':value.get('sessionId',path.parent.name)})
        self.db.execute('insert or replace into files values(?,?,?,?,?,?,?,?)',
                        (source, str(path), *meta, offset, json.dumps(state)))

    def opencode(self, path, rebuild=False):
        def metadata(p):
            try:
                s=p.stat(); return [s.st_dev, s.st_ino, s.st_size, s.st_mtime_ns]
            except FileNotFoundError:
                return None
        signature = [metadata(path), metadata(Path(str(path)+'-wal'))]
        previous = self.db.execute('select state from files where source=? and path=?', ('opencode',str(path))).fetchone()
        if not rebuild and previous and json.loads(previous[0]).get('database') == signature:
            return
        con = sqlite3.connect(path.resolve().as_uri() + '?mode=ro', uri=True)
        try:
            # time_updated is indexed by the native database only in some releases.
            # Reading scalar metadata also detects revisions with unchanged timestamps.
            for ident, sid, when, updated, raw in con.execute(
                    "select id,session_id,time_created,time_updated,data from session_message where type='assistant'"):
                row = json.loads(raw); u = row.get('tokens') or {}
                if not u or not (row.get('time') or {}).get('completed'):
                    continue
                model = row.get('model') or {}
                self.put('opencode', event(ident, when, model.get('id'),
                    {'input': u.get('input'), 'output': u.get('output'),
                     'cache_read': (u.get('cache') or {}).get('read', 0),
                     'cache_write': (u.get('cache') or {}).get('write', 0)},
                    native_cost=row.get('cost'), partial=bool((u.get('cache') or {}).get('write'))))
                self.bytes += len(raw)
            self.db.execute('insert or replace into files values(?,?,?,?,?,?,?,?)',
                            ('opencode',str(path),0,0,0,0,0,json.dumps({'database':signature})))
        finally:
            con.close()


def discover(root, source):
    patterns = {'codex': '**/*.jsonl', 'claude': '**/*.jsonl', 'codebuddy': '**/*.jsonl',
                'agy': 'brain/*/.system_generated/logs/transcript_full.jsonl',
                'grok': '*/*/usage.json'}
    if source == 'droid':
        return root.parent.joinpath('logs').glob('droid-log-single.log*')
    return root.glob(patterns[source])


def collect(ledger, config, agent='all', rebuild=False, ownership=None):
    result = {}
    for source, root in config.get('roots', {}).items():
        if source not in SOURCES or not root or agent not in ('all', source):
            continue
        try:
            version = {'agy': 2, 'droid': 3}.get(source, 1)
            stored = ledger.db.execute('select version from parser_versions where source=?',(source,)).fetchone()
            refresh_parser = rebuild or (stored[0] if stored else 1) != version
            if source == 'droid' and refresh_parser:
                ledger.db.execute("delete from files where source='droid' and path like '%.settings.json'")
            path = Path(root)
            if not path.exists():
                raise FileNotFoundError()
            if source == 'kiro':
                result[source] = {'note': '官方 Credits；原生未提供可定价的请求明细'}
                continue
            if source == 'opencode':
                ledger.opencode(path, rebuild)
            else:
                for item in discover(path, source):
                    ledger.file(source, item, 'droid_log' if source=='droid' else 'json' if source=='grok' else 'jsonl', refresh_parser)
                if source == 'codex' and config.get('codex_archive'):
                    for item in Path(config['codex_archive']).glob('**/*.jsonl'):
                        ledger.file(source, item, rebuild=rebuild)
                for item in (ownership.files.get(source, []) if ownership else []):
                    ledger.file(source, item, 'json' if source == 'grok' else 'jsonl', rebuild,
                                sid=ownership.identities.get((source,str(item))))
            ledger.db.execute('insert or replace into parser_versions values (?,?)',(source,version))
            ledger.db.commit()
            result[source] = {'captured_at': iso(time.time())}
        except (OSError, ValueError, sqlite3.Error, TypeError, AttributeError):
            ledger.db.rollback()
            result[source] = {'error': '采集失败，保留上次结果'}
    return result


def model_price(model, tokens, prices, now, aggregate=False, long_context=True):
    canonical = prices.get('aliases', {}).get(model, model)
    entries = prices.get('models', {}).get(canonical, [])
    if isinstance(entries, dict):
        entries = [entries]
    for price in reversed(entries):
        if price.get('until') and now >= stamp(price['until']):
            continue
        if price.get('from') and now < stamp(price['from']):
            continue
        context = sum(tokens.get(k, 0) for k in TOKEN_KEYS if k != 'output')
        # A turn may contain several short requests. Its sum cannot establish
        # that any individual request crossed the long-context price threshold.
        if aggregate and price.get('threshold') is not None and context > price['threshold']:
            return None
        rates = price.get('long') if long_context and price.get('threshold') is not None and context > price['threshold'] else price
        if not rates:
            return None
        if any(value and not number(rates.get(key)) for key, value in tokens.items()):
            return None
        return sum(value * rates.get(key, 0) for key, value in tokens.items()) / 1e6
    return None


def group_matches(source, bucket, model, item=None):
    if not model:
        return None
    group = str(bucket.get('group', '')).lower()
    if source == 'agy':
        if group in ('gemini models', 'gemini'):
            return model.startswith('gemini-')
        if group in ('claude and gpt models', 'claude/gpt'):
            return model.startswith(('claude-', 'gpt-'))
        return None
    if source == 'codex':
        if group == 'codex' or str(bucket.get('id', '')).startswith('codex:'):
            return True
        return None
    if source == 'droid':
        pool = str((item or {}).get('pool') or '').lower()
        if pool in ('standard','core','extra'):
            return pool == group
        # Closed models cannot overflow into the open-weight Core pool.
        if model.startswith(('claude-','gpt-','gemini-')):
            return group == 'standard'
        return None
    return True


def bounds(bucket, captured, overrides):
    end = stamp(bucket.get('resets_at'))
    override = overrides.get(str(bucket.get('id')), {})
    start = stamp(override.get('start')) if stamp(override.get('reset')) == end else None
    window = bucket.get('window')
    minutes = {'five_hour': 300, '5h': 300, 'weekly': 10080, 'seven_day': 10080, '7d': 10080}.get(window, window)
    if start is None and end and number(minutes):
        start = end - minutes * 60
    if start is None and window == 'monthly':
        # A reset date alone does not establish the provider's billing-cycle start.
        start = stamp(bucket.get('starts_at'))
    return start, end


def codex_sampling(ledger, ownership, bucket, start, end, until, used):
    points = []
    if start is not None and end is not None:
        for raw, in ledger.db.execute('select data from quota_events where source=? and at>=? and at<=? order by at', ('codex',start,until)):
            q = json.loads(raw)
            if q.get('end') is None or abs(q['end']-end)>20 or q.get('window') != bucket.get('window'):
                continue
            if q.get('group') != (bucket.get('group') or 'codex') or not ownership.contains('codex',q):
                continue
            points.append(q)
    # Never let a delayed lower percentage re-open a consumed quota window.
    full = next((q for q in points if q['used'] >= 100), None)
    cutoff = full['at'] if full else until
    if full:
        point = full
    elif points:
        peak = max(q['used'] for q in points)
        point = next(q for q in points if q['used'] == peak)
    else:
        point = None
    if point and point['used'] > 0:
        observed = min(100,point['used'])
        if used is not None and used < observed:
            return cutoff, None, '当前账号读数低于历史采样，需确认重置后的周期起点'
        rounded = 0 < observed < 100 and float(observed).is_integer()
        if rounded and not any(q['at'] < point['at'] and q['used'] < observed for q in points):
            return cutoff, None, '缺少同周期额度跳档记录，暂不估算'
        return cutoff, {'at':iso(point['at']), 'observed_percent':observed,
                        'percent':observed-.5 if rounded else observed, 'rounded':rounded}, None
    # Fractional official readings can be matched directly to their capture time.
    if used and not float(used).is_integer():
        return cutoff, {'at':iso(until),'observed_percent':used,'percent':used,'rounded':False}, None
    return cutoff, None, '已用 0%，暂不估算' if used == 0 else '缺少同周期额度跳档记录，暂不估算'


def load_prices(config):
    custom = read_json(config['prices'])
    default = read_json(config['default_prices'], {}) if config.get('default_prices') else {}
    if not default:
        return custom
    result = {**default, **custom}
    result['models'] = dict(default.get('models', {}))
    for model, rates in custom.get('models', {}).items():
        base = result['models'].get(model, {})
        result['models'][model] = {**base, **rates} if isinstance(base,dict) and isinstance(rates,dict) else rates
    result['aliases'] = {**default.get('aliases',{}), **custom.get('aliases',{})}
    if custom.get('version') != default.get('version'):
        result['version'] = f"{default.get('version')} / 配置 {custom.get('version')}"
    return result


def summarize(ledger, config, settings, prices, statuses, now, ownership=None):
    ownership = ownership or Ownership(config)
    rows = []
    counter_sessions = {sid for sid, in ledger.db.execute('select id from counter_sessions where source=?', ('codex',))}
    for source in config.get('roots', {}):
        if source not in SOURCES:
            continue
        quota = read_json(Path(config['quota_dir']) / f'{source}.json', {}) or {}
        captured = stamp(quota.get('captured_at'))
        buckets = [b for b in quota.get('buckets', []) if b.get('window') is not None]
        fallback = not buckets
        if fallback:
            buckets = [{'id': 'calendar', 'group': '', 'window': 'calendar',
                        'starts_at': datetime.fromtimestamp(now, UTC).replace(day=1,hour=0,minute=0,second=0,microsecond=0).isoformat()}]
        source_settings = settings.get('sources', {}).get(source, {})
        state = statuses.get(source, {})
        file_states = [json.loads(x[0]) for x in ledger.db.execute(
            'select state from files where source=?', (source,))]
        for bucket in buckets:
            start, end = bounds(bucket, captured, source_settings.get('windows', {}))
            if fallback:
                start, end = stamp(bucket['starts_at']), now
            until = min(now, captured) if captured and not fallback else now
            used = 100 - bucket['remaining_percent'] if number(bucket.get('remaining_percent')) else None
            if used is not None and not 0 <= used <= 100:
                used = None
            point, sampling_reason, exhausted_at = None, None, None
            if source == 'codex' and not fallback:
                cutoff, point, sampling_reason = codex_sampling(ledger, ownership, bucket, start, end, until, used)
                if cutoff < until:
                    exhausted_at = iso(cutoff)
                until = cutoff
            partial_files = any(s.get('partial') and (s.get('partial_unknown') or
                s.get('partial_at') is None or start is None or start <= s['partial_at'] <= until)
                and ownership.contains(source, {'sid':s.get('sid'), 'at':s.get('partial_at')}) for s in file_states)
            estimate_until = stamp(point['at']) if point else until
            estimate_amount = base_amount = range_extra = estimate_extra = 0
            by_model = {}
            unknown = 0
            multiple = source == 'codex' and len({b.get('group') for b in buckets}) > 1
            ambiguous = multiple
            if start is not None and end is not None:
                items = ledger.db.execute('select data from events e where source=? and (at is null or (at>=? and at<?)) and not exists (select 1 from excluded_events x where x.source=e.source and x.id=e.id)',
                                          (source, start, min(until + .000001, end)))
                for raw, in items:
                    item = json.loads(raw); model = item['model']
                    if source == 'codex' and item.get('meter') == 'request' and not item.get('reported') and item.get('sid') in counter_sessions:
                        continue
                    if not ownership.contains(source, item):
                        continue
                    if source == 'droid' and item.get('byok'):
                        continue
                    if source == 'droid' and not fallback:
                        item['pool'] = ledger.droid_pool(item)
                    match = None if multiple else True if fallback else group_matches(source, bucket, model,item)
                    if match is False:
                        continue
                    if match is None:
                        ambiguous = True
                        continue
                    part = by_model.setdefault(model, {'model': model, 'requests': 0, 'unpriced': 0, 'api_usd': 0, 'api_usd_max':0,
                                                       'tokens': dict.fromkeys(TOKEN_KEYS, 0)})
                    part['requests'] += 1
                    cost = None if item['partial'] or item['at'] is None else model_price(model, item['tokens'], prices, now, aggregate=source=='grok')
                    upper = cost
                    if cost is not None and item.get('cache_ttl_unknown'):
                        tokens = dict(item['tokens'])
                        tokens['cache_write_long'] += tokens['cache_write']; tokens['cache_write'] = 0
                        upper = model_price(model,tokens,prices,now)
                        if upper is None:
                            cost = None
                    if cost is None:
                        unknown += 1; part['unpriced'] += 1
                    else:
                        part['api_usd'] += cost
                        part['api_usd_max'] += upper
                        range_extra += upper - cost
                        base_amount += model_price(model, item['tokens'], prices, now, aggregate=source=='grok', long_context=False) or 0
                        if item['at'] <= estimate_until:
                            estimate_amount += cost
                            estimate_extra += upper - cost
                    for key, value in item['tokens'].items():
                        if number(value):
                            part['tokens'][key] += value
            parts = list(by_model.values())
            count = sum(p['requests'] for p in parts)
            priced = count - unknown
            amount = sum(p['api_usd'] for p in parts) if priced else None
            reason = state.get('error') or state.get('note')
            if not reason and (start is None or end is None): reason = '缺少周期边界，请设置本周期起点'
            missing_history = ownership.incomplete_since(source, start)
            log_start = min((s['first_at'] for s in file_states if s.get('first_at') is not None),default=None)
            missing_logs = source == 'droid' and start is not None and log_start is not None and start < log_start
            if not reason and missing_history: reason = '迁出历史缺失，不能完整核算本机期间用量'
            if not reason and missing_logs: reason = '日志未覆盖周期起点，仅统计保留的请求记录'
            if not reason and ambiguous: reason = '无法确认请求所属额度组'
            if not reason and unknown: reason = f'{unknown} 条用量缺少型号、价格或完整计量'
            if not reason and partial_files: reason = '历史记录不完整或累计计数缺少基线'
            if not reason and not priced: reason = '本周期没有可定价的用量记录'
            if not reason and (fallback or used is None): reason = '尚无官方已用百分比'
            if not reason and (quota.get('error') or not captured or now - captured > 900): reason = '官方额度已过期或查询失败'
            if not reason and end <= now: reason = '额度周期已结束，等待官方重置快照'
            if not reason and start > until: reason = '周期起点晚于采集时间'
            if not reason and used == 0: reason = '已用 0%，暂不估算'
            if not reason and sampling_reason: reason = sampling_reason
            denominator = point['percent'] if point else used
            estimate = estimate_amount * 100 / denominator if not reason and amount is not None else None
            if point:
                point['api_usd'] = estimate_amount
            rows.append({'source': source, 'id': bucket.get('id'), 'group': bucket.get('group'),
                         'window': bucket.get('window'), 'start': iso(start) if start is not None else None,
                         'end': iso(end) if end is not None else None, 'used_percent': used,
                         'api_usd': amount, 'equivalent_usd': estimate, 'reason': reason,
                         'api_usd_max':amount + range_extra if amount is not None and range_extra else None,
                         'equivalent_usd_max':estimate + estimate_extra * 100 / denominator if estimate is not None and estimate_extra else None,
                         'notes':['缓存写入未记录有效期，金额范围按 5 分钟至 1 小时单价计算'] if range_extra else [],
                         'base_api_usd':base_amount if priced else None, 'estimate_point':point,
                         'exhausted_at':exhausted_at, 'cost_until':iso(until),
                         'partial': bool(unknown or ambiguous or partial_files or missing_history or missing_logs or state.get('note') or state.get('error')),
                         'requests': count, 'unpriced': unknown, 'models': parts,
                         'quota_at': quota.get('captured_at'), 'captured_at': state.get('captured_at'),
                         'scope': '按迁移账本归属本机期间，包含迁出保留历史' + ('；Codex 按额度计量口径排除自动压缩和首次额度用完后的消费' if source == 'codex' else ''),
                         'local_sample': not source_settings.get('account_complete', False)})
    return {'version': 1, 'captured_at': iso(now), 'rows': rows,
            'price_version': prices.get('version'), 'price_sources': prices.get('sources', []),
            'basis': '按当前标准 API Token 单价估算，不含工具、图片等额外费用',
            'metrics': {'bytes_read': ledger.bytes, 'changed_files': ledger.changed}}


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('command', choices=('collect', 'report', 'reprice', 'rebuild'))
    ap.add_argument('--config', required=True, type=Path)
    ap.add_argument('--agent', choices=('all', *SOURCES), default='all')
    ap.add_argument('--format', choices=('json', 'table'), default='json')
    args = ap.parse_args()
    config = read_json(args.config)
    directory = Path(config['directory']); directory.mkdir(parents=True, exist_ok=True, mode=0o700)
    os.umask(0o077)
    with lock(directory / 'collector.lock'):
        ledger = Ledger(directory)
        try:
            settings = read_json(directory / 'settings.json', {}) or {}
            prices = load_prices(config)
            ownership = Ownership(config)
            statuses = read_json(directory / 'status.json', {}) or {}
            if args.command in ('collect', 'rebuild'):
                for source, status in collect(ledger, config, args.agent, args.command == 'rebuild', ownership).items():
                    if status.get('error'):
                        statuses[source] = {**statuses.get(source, {}), **status}
                    else:
                        statuses[source] = status
                save_json(directory / 'status.json', statuses)
            report = summarize(ledger, config, settings, prices, statuses, time.time(), ownership)
            save_json(directory / 'snapshot.json', report)
            if args.format == 'table':
                for row in report['rows']:
                    print(row['source'], row['group'], row['window'], row['api_usd'], row['used_percent'], row['equivalent_usd'], row['reason'] or '')
            else:
                print(json.dumps(report, ensure_ascii=False, allow_nan=False))
        finally:
            ledger.db.close()


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, sqlite3.Error, KeyError, TypeError):
        print('用量采集未完成；请检查配置、原生目录和 Python 环境。', file=sys.stderr)
        sys.exit(1)
