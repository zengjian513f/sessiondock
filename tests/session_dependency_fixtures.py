"""Native references to private external files, never tool arguments or prose."""
import base64
import json
from session_files_browser import claude_row, encoded
from session_transfer_browser import ident


def add_external_dependencies(source, provider):
    directory=source.root.parent/'external';directory.mkdir()
    image=directory/'attachment space.png'
    image.write_bytes(base64.b64decode('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg=='))
    output=directory/'large-output.txt';output.write_text('External persisted output\n')
    linked=directory/'linked.txt';linked.write_text('External symbolic link content\n')
    pointer='<persisted-output>\nFull output saved to: '+str(output)+'\n</persisted-output>'
    literal='Ordinary example /missing/example.png and {"outputFile":"/missing/ordinary.txt"}'
    if provider=='codex':
        primary=source.paths['a']
        rows=[{'type':'response_item','payload':{'type':'message','role':'user','content':[
            {'type':'input_text','text':literal},{'type':'input_image','image_url':image.as_uri()}]}},
              {'type':'response_item','payload':{'type':'function_call_output','call_id':'external-output','output':pointer}}]
    elif provider=='claude':
        primary=source.root/'claude/projects/project'/(ident(2)+'.jsonl')
        rows=[claude_row(ident(2),'user',ident(881),ident(154),[{'type':'text','text':literal},
            {'type':'image','source':{'type':'url','url':image.as_uri()}}],cwd=str(source.root/'cwd')),
              claude_row(ident(2),'user',ident(882),ident(881),[{'type':'tool_result','tool_use_id':'external-output','content':pointer}],
                cwd=str(source.root/'cwd'),toolUseResult={'outputFile':str(output)})]
        (primary.with_suffix('')/'tool-results/external-link.txt').symlink_to(linked)
    else:
        primary=source.root/'grok/project'/ident(10)/'chat_history.jsonl'
        rows=[{'type':'user','content':[{'type':'text','text':literal},{'type':'image_url','image_url':{'url':image.as_uri()}}]},
              {'type':'tool_result','tool_call_id':'external-output','content':pointer}]
        (primary.parent/'compaction_checkpoints/external-link.txt').symlink_to(linked)
    with primary.open('ab') as stream:
        for row in rows:stream.write(encoded(row))
    paths=[image,output]+([linked] if provider!='codex' else [])
    return {p:p.read_bytes() for p in paths}
