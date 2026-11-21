"""Official OpenAI Standard prices with a persistent last-successful cache."""
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
FIELDS = {'input':'input', 'output':'output', 'cached input':'cache_read',
          '30m write':'cache_write', '12h write':'cache_write_long'}


def parse_openai(text):
    section = re.search(r'^### Standard pricing data\s*\n(.*?)(?=^### |\Z)', text, re.M | re.S)
    if not section:
        raise ValueError('Standard pricing table missing')
    boundary = re.search(r'Short context:\s*(?:≤|<=)\s*([\d,.]+)\s*([KM]?) input tokens\.\s*'
                         r'Long context:\s*>\s*([\d,.]+)\s*([KM]?) input tokens', text, re.I)
    threshold = None
    if boundary:
        sizes = [float(boundary[i].replace(',', '')) * {'':1,'K':1000,'M':1000000}[boundary[i+1].upper()]
                 for i in (1,3)]
        if sizes[0] == sizes[1] and math.isfinite(sizes[0]) and sizes[0] > 0:
            threshold = sizes[0]
    models, headers = {}, None
    for line in section[1].splitlines():
        if not line.strip().startswith('|'):
            continue
        cells = [cell.strip().strip('`') for cell in line.strip().strip('|').split('|')]
        if all(re.fullmatch(r':?-+:?', cell) for cell in cells):
            continue
        if cells[0] == 'Model':
            headers = [cell.lower() for cell in cells]
            expected = ['model', *FIELDS] if 'input' in headers else [
                'model', *[prefix+name for prefix in ('short context ', 'long context ') for name in FIELDS]]
            # The plain table has no cache-write columns. Never confuse it with
            # Batch/Fast tables or guess a newly changed column layout.
            if headers != ['model','input','output','cached input'] and headers != expected:
                raise ValueError('Unknown Standard pricing columns')
            continue
        if headers is None or len(cells) != len(headers):
            raise ValueError('Incomplete Standard pricing row')
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
            if not {'input','output'} <= rates['long'].keys():
                raise ValueError('Incomplete long-context prices')
        if model in models:
            raise ValueError('Ambiguous Standard model prices')
        models[model] = rates
    if not models:
        raise ValueError('Empty Standard pricing table')
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
            result[key] = edits(item,baseline[key]) if isinstance(item,dict) and isinstance(baseline.get(key),dict) else item
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
    if enabled and refresh and (cache.get('url') != url or now-checked >= interval):
        try:
            request = Request(url, headers={'User-Agent':'SessionDock pricing sync', 'Accept':'text/markdown'})
            with urlopen(request, timeout=10) as response:
                models = parse_openai(response.read().decode('utf-8'))
            updated = {**cache, 'url':url, 'checked_at':now,
                       'updated_at':datetime.now(timezone.utc).isoformat(), 'error':None,
                       'models':{**cache.get('models',{}), **models}}
            save(cache_path, updated)
            cache = updated
        except (OSError, ValueError, HTTPException):
            # Do not overwrite prices or their successful timestamp on failure.
            cache = {**cache, 'url':url, 'checked_at':now,
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
    overrides = {key:edits(custom.get(key,{}),baseline.get(key,{})) for key in ('models','aliases')}
    pinned = read(directory/'price-overrides.json')
    result = {**default, 'models':{**default.get('models',{}), **cache.get('models',{})}}
    result = merge(merge(result,overrides), {k:pinned[k] for k in ('models','aliases') if k in pinned})
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
