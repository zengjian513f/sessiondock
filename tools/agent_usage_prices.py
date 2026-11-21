"""Official OpenAI token prices with a persistent last-successful cache."""
import json
import math
import re
import time
from datetime import datetime, timezone
from http.client import HTTPException
from pathlib import Path
from urllib.request import Request, urlopen


OPENAI_URL = 'https://developers.openai.com/api/docs/pricing.md'
REFRESH_SECONDS = 86400
RETRY_SECONDS = 3600
CACHE_SCHEMA = 2
ULTRAFAST_LIST_START = '2027-03-01T08:00:00Z'  # Midnight Pacific Standard Time.
FIELDS = {'input':'input', 'output':'output', 'cached input':'cache_read',
          '30m write':'cache_write', '12h write':'cache_write_long'}


def parse_openai(text):
    boundary = re.search(r'Short context:\s*(?:≤|<=)\s*([\d,.]+)\s*([KM]?) input tokens\.\s*'
                         r'Long context:\s*>\s*([\d,.]+)\s*([KM]?) input tokens', text, re.I)
    threshold = None
    if boundary:
        sizes = [float(boundary[i].replace(',', '')) * {'':1,'K':1000,'M':1000000}[boundary[i+1].upper()]
                 for i in (1,3)]
        if sizes[0] == sizes[1] and math.isfinite(sizes[0]) and sizes[0] > 0:
            threshold = sizes[0]
    sections = dict(re.findall(r'^### ([^\n]+?) pricing data[^\S\n]*\n(.*?)(?=^### |\Z)', text, re.M | re.S))
    models = parse_table(sections.get('Standard'), threshold)
    fast = parse_table(sections.get('Fast'), threshold)
    ultrafast = parse_table(sections.get('Ultrafast'), threshold)
    for label, target in (('Standard', models), ('Fast', fast)):
        target.update(parse_specialized_codex(text, label))
    if 'Ultrafast promotion' in sections or 'Ultrafast list' in sections:
        promotion = parse_table(sections.get('Ultrafast promotion'), threshold)
        regular = parse_table(sections.get('Ultrafast list'), threshold)
        if (promotion.keys() != regular.keys() or promotion.keys() & ultrafast.keys() or
                'through February 28, 2027' not in text or 'starting March 1, 2027 (Pacific time)' not in text):
            raise ValueError('Unknown Ultrafast promotion schedule')
        for model, rates in promotion.items():
            ultrafast[model] = [
                {**rates, 'until': ULTRAFAST_LIST_START},
                {**regular[model], 'from': ULTRAFAST_LIST_START},
            ]
    return models, {'fast': fast, 'ultrafast': ultrafast}


def parse_specialized_codex(text, label):
    # The older Codex model is listed separately under Specialized models.
    specialized = text.partition('\nSpecialized models\n')[2].partition('\nTools\n')[0]
    match = re.search(rf'^{label}[^\S\n]*\n\s*### Grouped Pricing Table data\s*\n(.*?)(?=^### |\Z)',
                      specialized, re.M | re.S)
    if not match:
        return {}
    table = [line.strip().split('|', 2)[-1] for line in match[1].splitlines()
             if line.startswith(('| Category |', '| Codex |'))]
    if len(table) < 2:
        return {}
    return parse_table('\n'.join('|' + row for row in table), None)


def parse_table(section, threshold):
    if section is None:
        raise ValueError('Pricing table missing')
    models, headers = {}, None
    for line in section.splitlines():
        if not line.strip().startswith('|'):
            continue
        cells = [cell.strip().strip('`') for cell in line.strip().strip('|').split('|')]
        if all(re.fullmatch(r':?-+:?', cell) for cell in cells):
            continue
        if cells[0] == 'Model':
            headers = [cell.lower() for cell in cells]
            expected = ['model', *FIELDS] if 'input' in headers else [
                'model', *[prefix+name for prefix in ('short context ', 'long context ') for name in FIELDS]]
            # Tables without a context split may omit cache-write columns.
            if headers != ['model','input','output','cached input'] and headers != expected:
                raise ValueError('Unknown pricing columns')
            continue
        if headers is None or len(cells) != len(headers):
            raise ValueError('Incomplete pricing row')
        model = cells[0].split(' (',1)[0]
        if not re.fullmatch(r'[a-z0-9][a-z0-9_.-]*', model):
            raise ValueError('Unknown model label')
        values = {}
        for column, cell in zip(headers[1:], cells[1:]):
            if cell == '-':
                continue
            if not re.fullmatch(r'\$[\d,]+(?:\.\d+)?', cell):
                raise ValueError('Unknown price')
            value = float(cell[1:].replace(',', ''))
            if not math.isfinite(value):
                raise ValueError('Invalid price')
            values[column] = value
        long_context = 'short context input' in headers
        prefix = 'short context ' if long_context else ''
        rates = {key:values[prefix+name] for name,key in FIELDS.items() if prefix+name in values}
        if not {'input','output'} <= rates.keys():
            raise ValueError('Incomplete token prices')
        if long_context:
            if threshold is None:
                raise ValueError('Long-context boundary missing')
            rates['threshold'] = threshold
            rates['long'] = {key:values['long context '+name] for name,key in FIELDS.items()
                             if 'long context '+name in values}
            if rates['long'] and not {'input','output'} <= rates['long'].keys():
                raise ValueError('Incomplete long-context prices')
        if model in models:
            raise ValueError('Ambiguous model prices')
        models[model] = rates
    if not models:
        raise ValueError('Empty pricing table')
    return models


def read(path):
    try:
        value = json.loads(path.read_text(encoding='utf-8'))
        return value if isinstance(value, dict) else {}
    except (OSError, ValueError):
        return {}


def merge(base, overlay):
    result = dict(base)
    for key,value in overlay.items():
        result[key] = merge(result[key], value) if isinstance(result.get(key),dict) and isinstance(value,dict) else value
    return result


def edits(value, baseline):
    result = {}
    for key,item in value.items():
        if key not in baseline or item != baseline[key]:
            if isinstance(item,dict) and isinstance(baseline.get(key),dict):
                changed = edits(item,baseline[key])
                if changed:
                    result[key] = changed
            else:
                result[key] = item
    return result


def update_tiers(previous, fresh):
    result = {}
    for tier, models in fresh.items():
        combined = dict(previous.get(tier, {}))
        for model, rates in models.items():
            old = combined.get(model)
            # A later catalog may fold the regular price into the main table.
            # Retain the published promotion window for requests made before it ended.
            if isinstance(old, list) and isinstance(rates, dict):
                ended = [entry for entry in old if 'until' in entry]
                if ended:
                    rates = [*ended, {**rates, 'from': ended[-1]['until']}]
            combined[model] = rates
        result[tier] = combined
    return result


def synced_prices(config, default, custom, save, refresh=False):
    directory = Path(config['directory'])
    options = config['price_sync']
    cache_path = directory/'online-prices.json'
    cache = read(cache_path)
    enabled = options.get('enabled', True)
    url = options.get('url') or OPENAI_URL
    now = time.time()
    checked = cache.get('checked_at', 0)
    interval = RETRY_SECONDS if cache.get('error') else REFRESH_SECONDS
    if enabled and refresh and (cache.get('schema') != CACHE_SCHEMA or cache.get('url') != url
                                or now-checked >= interval):
        try:
            request = Request(url, headers={'User-Agent':'SessionDock pricing sync', 'Accept':'text/markdown'})
            with urlopen(request, timeout=10) as response:
                models, tiers = parse_openai(response.read().decode('utf-8'))
            updated = {**cache, 'schema':CACHE_SCHEMA, 'url':url, 'checked_at':now,
                       'updated_at':datetime.now(timezone.utc).isoformat(), 'error':None,
                       'models':{**cache.get('models',{}), **models},
                       'tiers':update_tiers(cache.get('tiers', {}), tiers)}
            save(cache_path, updated)
            cache = updated
        except (OSError, ValueError, HTTPException):
            # Do not overwrite prices or their successful timestamp on failure.
            cache = {**cache, 'schema':CACHE_SCHEMA, 'url':url, 'checked_at':now,
                     'error':'OpenAI 官方价目同步失败，沿用上次成功价格；尚无缓存时使用内置价目'}
            try:
                save(cache_path, cache)
            except OSError:
                pass
    baseline_path = directory/'price-base.json'
    baseline = read(baseline_path)
    if not baseline:
        baseline = default
        save(baseline_path, baseline)
    # Old releases copied all defaults into prices.json. Only actual edits to
    # that copy should override live prices; leave the private file untouched.
    keys = ('models', 'aliases', 'tiers')
    overrides = {key:edits(custom.get(key,{}),baseline.get(key,{})) for key in keys}
    pinned = read(directory/'price-overrides.json')
    result = {**default, 'models':{**default.get('models',{}), **cache.get('models',{})}}
    result['tiers'] = merge(default.get('tiers', {}), cache.get('tiers', {}))
    result = merge(merge(result,overrides), {k:pinned[k] for k in keys if k in pinned})
    result['version'] = default.get('version','内置')
    if cache.get('updated_at'):
        result['version'] += ' / OpenAI '+cache['updated_at']
    if any(overrides.values()) or pinned:
        result['version'] += ' / 本地覆盖'
    result['sync'] = {'enabled':enabled, 'provider':'OpenAI', 'updated_at':cache.get('updated_at'),
                      'checked_at':cache.get('checked_at'), 'error':cache.get('error'),
                      'cached':bool(cache.get('models'))}
    result['sources'] = list(dict.fromkeys([*default.get('sources',[]),OPENAI_URL]))
    return result
