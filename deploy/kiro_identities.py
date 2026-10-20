#!/usr/bin/env python3
"""Preserve existing SessionDock Kiro UIDs before publishing the ACP catalog.

Reads the running service's directory API, never native session files. Writes
only a private identity receipt under the existing deployment state directory.
Does not publish artifacts, restart services, or read conversation bodies.
"""
from __future__ import annotations

import argparse
import base64
from concurrent.futures import ThreadPoolExecutor
import json
from pathlib import Path
import shlex

from sdtargets import Shell, load_targets

# Executed on the target, where its loopback service and state live.
CAPTURE = r'''
import json, os, pathlib, socket, tempfile, urllib.request
configuration = CONFIGURATION
expected = configuration["hostname"]
actual = socket.gethostname()
if actual.casefold() != expected.casefold():
    raise RuntimeError("target identity mismatch: " + actual)
url = configuration["health_url"].split("/api/meta", 1)[0] + "/api/sessions?force=1"
with urllib.request.urlopen(url, timeout=12) as response:
    inventory = json.load(response)
if not isinstance(inventory.get("sessions"), list):
    raise RuntimeError("service directory did not return sessions")
destination = pathlib.Path(configuration["prefix"]) / "state" / "kiro-legacy-identities.json"
previous = json.loads(destination.read_text()) if destination.exists() else []
identities = {row["uid"]: row for row in previous}
def collect(rows):
    for row in rows:
        if row.get("source") == "kiro":
            uid, sid = row.get("uid"), row.get("sid") or row.get("session_id")
            if not isinstance(uid, str) or not isinstance(sid, str) or not sid:
                raise RuntimeError("Kiro directory row lacks explicit identity")
            identities[uid] = {"source":"kiro", "uid":uid, "sid":sid, "cwd":row.get("cwd", "")}
        collect(row.get("agent_items") or [])
collect(inventory["sessions"])
if not destination.parent.is_dir():
    raise RuntimeError("deployment state directory is missing")
fd, temporary = tempfile.mkstemp(prefix=".kiro-identities-", dir=destination.parent)
try:
    with os.fdopen(fd, "w") as output:
        json.dump(list(identities.values()), output)
        output.flush()
        os.fsync(output.fileno())
    os.replace(temporary, destination)
    if os.name == "posix":
        directory = os.open(destination.parent, os.O_RDONLY | os.O_DIRECTORY)
        try: os.fsync(directory)
        finally: os.close(directory)
finally:
    if os.path.exists(temporary): os.unlink(temporary)
print(json.dumps({"hostname":actual, "identities":len(identities), "status":"saved"}))
'''


def command(target):
    configuration = {
        "hostname": target.extra.get("expected_hostname") or target.name,
        "health_url": target.health_url,
        "prefix": target.prefix,
    }
    script = CAPTURE.replace("CONFIGURATION", repr(configuration), 1)
    encoded = base64.b64encode(script.encode()).decode()
    bootstrap = f"import base64;exec(base64.b64decode('{encoded}'))"
    if target.kind.startswith("windows"):
        return 'python -c "' + bootstrap + '"'
    return "python3 -c " + shlex.quote(bootstrap)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--targets-file", type=Path, default=Path(__file__).with_name("targets.local.json"))
    parser.add_argument("--targets", required=True)
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()
    names = set(args.targets.split(","))
    targets = load_targets(args.targets_file)
    missing = names - {target.name for target in targets}
    if missing:
        parser.error("unknown targets: " + ",".join(sorted(missing)))

    def capture(target):
        if not target.enabled:
            return target.name, "disabled", ""
        if target.kind in ("hub", "resource-agent"):
            return target.name, "excluded", "component has no local Kiro catalog"
        if args.dry_run:
            return target.name, "planned", "verify identity; read directory API; merge private UID/SID receipt"
        rc, output = Shell(target).run(command(target), timeout=30)
        return target.name, "saved" if rc == 0 else "pending", output.strip()[-1500:]

    with ThreadPoolExecutor(max_workers=8) as pool:
        results = list(pool.map(capture, [target for target in targets if target.name in names]))
    for name, status, detail in results:
        print(f"{name}: {status}" + (f" — {detail}" if detail else ""))
    return int(any(status == "pending" for _, status, _ in results))


if __name__ == "__main__":
    raise SystemExit(main())
