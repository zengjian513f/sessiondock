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


def parse_record(source, row, state, path, offset):
    """Return only scalar metering fields; never store conversation text."""
    if not isinstance(row, dict):
        return []
    kind = row.get('type')
    if source == 'codex':
        p = row.get('payload') or {}
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
        return [event(ident, row.get('timestamp'), state.get('model'),
                      {'input': u.get('input_tokens'), 'output': u.get('output_tokens'),
                       'cache_read': u.get('cached_input_tokens', 0),
                       'cache_write': u.get('cache_write_input_tokens', 0)}, included=True,
                      partial=bool(u.get('cache_write_input_tokens')))]
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
    if source == 'agy' and kind == 'PLANNER_RESPONSE':
        if row.get('status') != 'DONE' or 'input_tokens' not in row:
            return []
        sid = path.parents[2].name
        return [event(f"{sid}:{row.get('step_index',offset)}", row.get('created_at'), row.get('model'),
                      {'input': row.get('input_tokens'), 'output': row.get('output_tokens'),
                       'cache_read': row.get('cache_read_tokens', 0)})]
    return []


class Ledger:
    def __init__(self, directory):
        self.db = sqlite3.connect(directory / 'ledger.sqlite')
        self.db.execute('pragma journal_mode=WAL')
        self.db.executescript('''
            create table if not exists events(source text, id text, at real, data text,
              primary key(source,id));
            create index if not exists events_time on events(source,at);
            create table if not exists files(source text,path text,device int,inode int,size int,
              mtime int,offset int,state text,primary key(source,path));
        ''')
        self.bytes = 0
        self.changed = 0

    def put(self, source, item):
        self.db.execute('insert or replace into events values (?,?,?,?)',
                        (source, item['id'], item['at'], json.dumps(item)))

    def file(self, source, path, kind='jsonl', rebuild=False):
        stat = path.stat()
        previous = self.db.execute('select device,inode,size,mtime,offset,state from files where source=? and path=?',
                                   (source, str(path))).fetchone()
        meta = (stat.st_dev, stat.st_ino, stat.st_size, stat.st_mtime_ns)
        if not rebuild and previous and tuple(previous[:4]) == meta:
            return
        state, offset = {}, 0
        if kind == 'jsonl' and not rebuild and previous and tuple(previous[:2]) == meta[:2] and stat.st_size > previous[2]:
            offset, state = previous[4], json.loads(previous[5])
        self.changed += 1
        if kind == 'jsonl':
            with path.open('rb') as stream:
                stream.seek(offset)
                while True:
                    raw = stream.readline()
                    self.bytes += len(raw)
                    if not raw or not raw.endswith(b'\n'):
                        break
                    try:
                        row = json.loads(raw)
                        for item in parse_record(source, row, state, path, offset):
                            self.put(source, item)
                    except (ValueError, TypeError, AttributeError):
                        incomplete(state)
                    offset = stream.tell()
        elif source == 'grok':
            value = read_json(path); self.bytes += stat.st_size
            for turn in value.get('turns', []):
                for model, u in (turn.get('modelUsage') or {}).items():
                    ident = f"{value.get('sessionId',path.parent.name)}:{turn.get('turnNumber')}:{model}"
                    self.put(source, event(ident, turn.get('endedAt'), model,
                        {'input': u.get('inputTokens'), 'output': u.get('outputTokens'),
                         'cache_read': u.get('cachedReadTokens', 0), 'cache_write': u.get('cacheCreationTokens', 0)},
                        included=True, native_cost=u['costUsdTicks'] / 1e10 if number(u.get('costUsdTicks')) else None,
                        partial=bool(turn.get('usageIsIncomplete') or turn.get('costIsPartial'))))
        elif source == 'droid':
            # Settings contain cumulative session totals without per-call time/model.
            # Preserve an observation baseline; never turn the entire total into a bill.
            value = read_json(path); self.bytes += stat.st_size
            state = {'baseline_at': iso(time.time()), 'total': value.get('tokenUsage'), 'partial': True}
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
                'grok': '*/*/usage.json', 'droid': '**/*.settings.json'}
    return root.glob(patterns[source])


def collect(ledger, config, agent='all', rebuild=False):
    result = {}
    for source, root in config.get('roots', {}).items():
        if source not in SOURCES or not root or agent not in ('all', source):
            continue
        try:
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
                    ledger.file(source, item, 'json' if source in ('grok', 'droid') else 'jsonl', rebuild)
            ledger.db.commit()
            result[source] = {'captured_at': iso(time.time())}
            if source == 'droid':
                result[source]['note'] = '原生只有会话累计量，无法准确分配到额度周期'
        except (OSError, ValueError, sqlite3.Error, TypeError, AttributeError):
            ledger.db.rollback()
            result[source] = {'error': '采集失败，保留上次结果'}
    return result


def model_price(model, tokens, prices, now, aggregate=False):
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
        rates = price.get('long') if price.get('threshold') is not None and context > price['threshold'] else price
        if not rates:
            return None
        if any(value and not number(rates.get(key)) for key, value in tokens.items()):
            return None
        return sum(value * rates.get(key, 0) for key, value in tokens.items()) / 1e6
    return None


def group_matches(source, bucket, model):
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
        return None  # Standard/Core overflow routing is not present in native history.
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


def summarize(ledger, config, settings, prices, statuses, now):
    rows = []
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
            partial_files = any(s.get('partial') and (s.get('partial_unknown') or
                s.get('partial_at') is None or start is None or s['partial_at'] >= start) for s in file_states)
            until = min(now, captured) if captured and not fallback else now
            used = 100 - bucket['remaining_percent'] if number(bucket.get('remaining_percent')) else None
            if used is not None and not 0 <= used <= 100:
                used = None
            by_model = {}
            unknown = 0
            multiple = source == 'codex' and len({b.get('group') for b in buckets}) > 1
            ambiguous = multiple
            if start is not None and end is not None:
                items = ledger.db.execute('select data from events where source=? and (at is null or (at>=? and at<?))',
                                          (source, start, min(until + .000001, end)))
                for raw, in items:
                    item = json.loads(raw); model = item['model']
                    match = None if multiple else True if fallback else group_matches(source, bucket, model)
                    if match is False:
                        continue
                    if match is None:
                        ambiguous = True
                        continue
                    part = by_model.setdefault(model, {'model': model, 'requests': 0, 'unpriced': 0, 'api_usd': 0,
                                                       'tokens': dict.fromkeys(TOKEN_KEYS, 0)})
                    part['requests'] += 1
                    cost = None if item['partial'] or item['at'] is None else model_price(model, item['tokens'], prices, now, aggregate=source=='grok')
                    if cost is None:
                        unknown += 1; part['unpriced'] += 1
                    else:
                        part['api_usd'] += cost
                    for key, value in item['tokens'].items():
                        if number(value):
                            part['tokens'][key] += value
            parts = list(by_model.values())
            count = sum(p['requests'] for p in parts)
            priced = count - unknown
            amount = sum(p['api_usd'] for p in parts) if priced else None
            reason = state.get('error') or state.get('note')
            if not reason and (start is None or end is None): reason = '缺少周期边界，请设置本周期起点'
            if not reason and ambiguous: reason = '无法确认请求所属额度组'
            if not reason and unknown: reason = f'{unknown} 条用量缺少型号、价格或完整计量'
            if not reason and partial_files: reason = '历史记录不完整或累计计数缺少基线'
            if not reason and not priced: reason = '本周期没有可定价的用量记录'
            if not reason and (fallback or used is None): reason = '尚无官方已用百分比'
            if not reason and (quota.get('error') or not captured or now - captured > 900): reason = '官方额度已过期或查询失败'
            if not reason and end <= now: reason = '额度周期已结束，等待官方重置快照'
            if not reason and start > until: reason = '周期起点晚于采集时间'
            if not reason and used == 0: reason = '已用 0%，暂不估算'
            estimate = amount * 100 / used if not reason and amount is not None else None
            rows.append({'source': source, 'id': bucket.get('id'), 'group': bucket.get('group'),
                         'window': bucket.get('window'), 'start': iso(start) if start is not None else None,
                         'end': iso(end) if end is not None else None, 'used_percent': used,
                         'api_usd': amount, 'equivalent_usd': estimate, 'reason': reason,
                         'partial': bool(unknown or ambiguous or partial_files or state.get('note') or state.get('error')),
                         'requests': count, 'unpriced': unknown, 'models': parts,
                         'quota_at': quota.get('captured_at'), 'captured_at': state.get('captured_at'),
                         'scope': '本机已记录用量；跨设备、网页和未记录消费未计入',
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
            prices = read_json(config['prices'])
            statuses = read_json(directory / 'status.json', {}) or {}
            if args.command in ('collect', 'rebuild'):
                for source, status in collect(ledger, config, args.agent, args.command == 'rebuild').items():
                    if status.get('error'):
                        statuses[source] = {**statuses.get(source, {}), **status}
                    else:
                        statuses[source] = status
                save_json(directory / 'status.json', statuses)
            report = summarize(ledger, config, settings, prices, statuses, time.time())
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
