#!/usr/bin/env python3
"""Run the repository's reviewed browser scenarios and preserve current-run evidence.

# run_validation: skip
"""
# run_validation: skip
import argparse
from datetime import datetime, timezone
import json
from pathlib import Path
import subprocess
import sys
import traceback

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_SUITES = [
    'sidebar_input_browser',
    'startup_actions_browser',
    'audit_shutdown_browser',
    'runtime_log_fault_browser',
    'audit_delivery_browser',
    'diagnostic_retention_browser',
    'hub_diagnostics_browser',
    'terminal_log_volume_browser',
    'groups_browser',
    'cli_diagnostics_browser',
    'content_scroll_browser',
    'toolbar_options_browser',
    'session_relations_browser',
    'frontend_framework_browser',
    'new_session_model_browser',
    'frontend_search_state_browser',
    'regex_worker_browser',
    'trash_interactions_browser',
    'bug_report_node_browser',
    'composer_editor_browser',
    'files_browser',
    'hub_transfer_browser',
    'session_cleanup_browser',
    'session_freeze_browser',
    'session_stop_browser',
    'search_browser',
    'session_research_browser',
    'history_browser',
    'history_pages_browser',
    'terminal_input_browser',
    'terminal_selection_browser',
    'terminal_shift_browser',
    'terminal_claim_browser',
    'terminal_final_screen_browser',
    'terminal_scrollback_browser',
    'session_resources_browser',
    'resource_probe_browser',
    'popup_browser',
    'metadata_browser',
    'network_recovery_browser',
    'session_link_reveal_browser',
    'agent_menu_browser',
    'cli_menus_browser',
    'client_update_browser',
    'machine_controls_browser',
    'native_tags_browser',
    'nest_tree_browser',
    'rich_tools_browser',
    'history_pages_resume_browser',
    'session_link_lineage_browser',
    'pick_drag_browser',
    'session_continue_browser',
    'page_sleep_browser',
    'media_lazy_browser',
    'question_browser',
    'hub_bulk_browser',
    'session_tree_delete_browser',
    'bug_report_upload_browser',
    'attachment_resume_browser',
    'draft_sync_browser',
    'prefs_migration_browser',
    'hub_browser',
    'new_session_follow_browser',
    'send_browser',
]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=ROOT/'target/release/sessiondock')
    parser.add_argument('--output', type=Path, default=ROOT/'target/functional-review')
    parser.add_argument('--only', help='Comma-separated repository browser suite names')
    parser.add_argument('--jobs', type=int, default=2)
    args = parser.parse_args()
    args.output = args.output.resolve()
    args.output.mkdir(parents=True, exist_ok=True)
    catalog = json.loads((ROOT/'tests/functional_operations.json').read_text())
    suites = args.only.split(',') if args.only else DEFAULT_SUITES
    assert all(s.endswith('_browser') and (ROOT/'tests'/f'{s}.py').is_file() for s in suites), suites
    started = datetime.now(timezone.utc).isoformat()
    report_path = args.output/'coverage.json'
    report = {'status':'running', 'started':started, 'operations':[
        dict(o, status='not executed in current run', evidence=[]) for o in catalog['operations']]}
    report_path.write_text(json.dumps(report, ensure_ascii=False, indent=2)+'\n')
    (args.output/'suites.json').write_text(json.dumps({'status':'running', 'results':[]})+'\n')
    command = [sys.executable, str(ROOT/'tests/run_validation.py'), '--only', ','.join(suites),
        '--binary', str(args.binary.resolve()), '--browser-jobs', str(args.jobs), '--keep-going',
        '--json', str(args.output/'suites.json'), '--log-dir', str(args.output/'logs')]
    try:
        with (args.output/'runner.log').open('w') as log:
            with subprocess.Popen(command, cwd=ROOT, stdout=subprocess.PIPE,
                    stderr=subprocess.STDOUT, text=True) as result:
                for line in result.stdout:
                    log.write(line); log.flush()
                    print(line, end='', flush=True)
                result.wait()
        summary = json.loads((args.output/'suites.json').read_text())
    except BaseException:
        failure = traceback.format_exc()
        (args.output/'failure.txt').write_text(failure)
        report.update(status='failed', failure=failure, finished=datetime.now(timezone.utc).isoformat())
        report_path.write_text(json.dumps(report, ensure_ascii=False, indent=2)+'\n')
        raise
    executed = {r['name']:r for r in summary.get('results',[])}
    for operation in report['operations']:
        matches = [executed[s] for s in operation['suites'] if s in executed]
        if matches:
            operation['status'] = 'named scenario passed; remaining scope explicit' if all(
                s['status']=='PASS' for s in matches) else 'current scenario failed'
            operation['evidence'] = [s['log'] for s in matches]
    report.update(status='passed' if result.returncode==0 else 'failed', exit_code=result.returncode,
        finished=datetime.now(timezone.utc).isoformat())
    report_path.write_text(json.dumps(report, ensure_ascii=False, indent=2)+'\n')
    return result.returncode


if __name__ == '__main__':
    sys.exit(main())
