"""Kiro IDE metering from completed turns, retaining only scalar evidence."""
from collections import defaultdict
import json


CACHE_FACTOR = 0.375
# Published Kiro context windows for the model IDs that report percentages.
CONTEXT_WINDOWS = {
    'claude-opus-5-5':1000000, 'claude-sonnet-5-5':1000000, 'claude-haiku-5-5':1000000,
    'claude-opus-5':1000000, 'claude-opus-4-8':1000000, 'claude-opus-4-7':1000000,
    'claude-opus-4-6':1000000, 'claude-sonnet-5':1000000, 'claude-sonnet-4-6':1000000,
    'claude-fable-5-1':1000000, 'claude-opus-4-5':200000, 'claude-sonnet-4-5':200000,
    'claude-haiku-4-5':200000, 'gpt-5.6-sol':1000000, 'gpt-5.6-terra':1000000,
    'gpt-5.6-luna':1000000, 'kimi-k3':1000000,
}


def model_id(value):
    model = value.split('::', 1)[-1].lower() if isinstance(value, str) else ''
    if model.startswith('claude-'):
        model = model.replace('.', '-')
    return model


def text_units(value):
    if not isinstance(value, str):
        value = json.dumps(value, ensure_ascii=False, separators=(',', ':'))
    # Four ASCII characters or one non-ASCII character per estimated token.
    return len(value) + 3 * sum(not ch.isascii() for ch in value)


class Usage:
    def __init__(self):
        self.input = self.output = self.history = 0
        self.last_generation = None
        self.contexts = []
        self.models = set()
        self.tools = set()

    def add(self, payload, kind, at):
        model = model_id(payload.get('reasoningModelId'))
        if model:
            self.models.add(model)
        if kind == 'session_metadata' and payload.get('key') == 'contextUsage':
            value = payload.get('value')
            if isinstance(value, dict):
                self.contexts.append((value.get('usedTokens'), value.get('maxTokens'), value.get('usagePercentage')))
        if kind == 'tool_call':
            tool = payload.get('toolCallId')
            if not tool or tool in self.tools:
                return
            self.tools.add(tool)
            units = text_units(payload.get('args', {}))
        elif kind in ('assistant', 'tool_result', 'user'):
            if payload.get('operationType') == 'Summary':
                return
            units = text_units(payload.get('content', ''))
        else:
            return
        if kind in ('assistant', 'tool_call'):
            # Subtask files often omit context samples. Infer generation
            # bursts from adjacency and approximate each burst's input using
            # the visible transcript accumulated up to that point.
            if self.last_generation is None or at - self.last_generation > 1:
                self.input += self.history
            self.last_generation = at
            self.output += units
        self.history += units

    def tokens(self, fallback, number, subtask=False):
        model = next(iter(self.models), fallback) if len(self.models) <= 1 else ''
        if not model:
            return None
        prompt = 0
        for used, limit, percent in self.contexts:
            if number(used):
                prompt += used
            elif number(percent) and percent <= 100:
                if not number(limit) or not limit:
                    limit = CONTEXT_WINDOWS.get(model)
                if not limit:
                    return None
                prompt += limit * percent / 100
            else:
                return None
        if not self.contexts:
            if not subtask or self.last_generation is None:
                return None
            prompt = self.input / 4
        return {'model':model, 'tokens':{'input':prompt, 'output':self.output / 4}}


def metered_turns(metadata, main, subtasks, stamp, number):
    turns, summaries, children = defaultdict(Usage), {}, defaultdict(dict)
    seen = set()
    for row in main:
        ident, at = row.get('id'), stamp(row.get('timestamp'))
        if not isinstance(ident, str) or ident in seen or at is None:
            continue
        seen.add(ident)
        payload = row.get('payload') or {}
        kind = payload.get('type')
        execution = payload.get('parentExecutionId') if kind == 'sub_agent_start' else payload.get('executionId')
        if not isinstance(execution, str) or not execution:
            continue
        if kind == 'sub_agent_start' and isinstance(payload.get('subSessionId'), str):
            children[execution][payload['subSessionId']] = text_units(payload.get('prompt', ''))
        elif kind == 'usage_summary':
            credits = [p.get('usage') for p in payload.get('promptTurnSummaries', [])
                       if p.get('unit') == 'credit']
            if credits and all(number(value) for value in credits) and sum(credits) > 0:
                summaries[execution] = (at, sum(credits))
        else:
            turns[execution].add(payload, kind, at)

    # Each child file belongs to a main execution recorded by sub_agent_start.
    # The main usage_summary supplies the charge for the entire execution.
    for execution, (at, credits) in summaries.items():
        fallback = model_id(metadata.get('modelId'))
        main_tokens = turns[execution].tokens(fallback, number)
        parts = [main_tokens]
        for child, prompt in sorted(children[execution].items()):
            usage = Usage()
            usage.history = prompt
            for row in subtasks(child):
                ident, at_child = row.get('id'), stamp(row.get('timestamp'))
                if not isinstance(ident, str) or ident in seen or at_child is None:
                    continue
                seen.add(ident)
                payload = row.get('payload') or {}
                if payload.get('executionId') == execution:
                    usage.add(payload, payload.get('type'), at_child)
            parts.append(usage.tokens(main_tokens['model'] if main_tokens else fallback, number, subtask=True))
        yield {'id':execution, 'sid':metadata.get('id'), 'at':at, 'credits':credits, 'parts':parts,
               'model':main_tokens['model'] if main_tokens else fallback}
