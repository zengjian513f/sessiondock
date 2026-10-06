"""Shared fixture helpers extracted from the former `session_clone_service_real.py` (removed 2026-10-06
with the non-browser suites); imported by browser suites."""
# run_validation: skip
import copy


def remap_pages(pages, ids):
    result=copy.deepcopy(pages)
    for turn in result:
        turn['id']=ids['turns'][turn['id']]
        for item in turn['items']:
            item['id']=ids['records'][item['id']]
            if item['type']=='collabAgentToolCall':
                item['senderThreadId']=ids['threads'][item['senderThreadId']]
                item['receiverThreadIds']=[ids['threads'][v] for v in item['receiverThreadIds']]
                item['agentsStates']={ids['threads'][k]:v for k,v in item['agentsStates'].items()}
                for agent in item.get('receiverAgents',[]):agent['threadId']=ids['threads'][agent['threadId']]
    return result
