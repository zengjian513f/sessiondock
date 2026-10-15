"""Failure-safe evidence for browser suites; no dated or machine-local dependencies."""
import json
from pathlib import Path
import sys
import traceback
from datetime import datetime, timezone


def run_with_evidence(main, default_output):
    output = Path(default_output)
    if '--output' in sys.argv:
        output = Path(sys.argv[sys.argv.index('--output') + 1])
    output.mkdir(parents=True, exist_ok=True)
    result = output / 'result.json'
    started = datetime.now(timezone.utc).isoformat()
    # Invalidate a previous PASS before starting any fixture or browser.
    result.write_text(json.dumps({'status': 'running', 'started': started}) + '\n')
    try:
        main()
    except BaseException:
        failure = traceback.format_exc()
        (output / 'failure.txt').write_text(failure)
        result.write_text(json.dumps({'status': 'failed', 'started': started,
            'failure': failure}, indent=2) + '\n')
        raise
    evidence = json.loads(result.read_text())
    evidence.update(status='passed', started=started, finished=datetime.now(timezone.utc).isoformat())
    result.write_text(json.dumps(evidence, indent=2) + '\n')
