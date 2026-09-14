#!/usr/bin/env python3
"""Directory browser grants survive rename/delete and restart.
Only temporary native histories and files are used; no real CLI is started.
"""
import argparse
import json
from pathlib import Path
import tempfile
from urllib.parse import urlencode
from urllib.request import Request
from urllib.error import HTTPError

from history_parity import BINARY, build_corpus, claude_row, isolated_server


def request(opener, base, uid, reference, *, path=None, mode=None, action=None):
    body = {"uid": uid, "ref": str(reference)}
    if action is not None:
        req = Request(base + "/api/session/files/action", data=json.dumps({**body, **action}).encode(),
                      headers={"Content-Type": "application/json"})
    else:
        if path is not None:
            body["path"] = str(path)
        if mode:
            body["mode"] = mode
        req = Request(base + "/api/session/files?" + urlencode(body))
    try:
        response = opener.open(req, timeout=10)
    except HTTPError as error:
        response = error
    with response:
        return response.status, json.load(response)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sessiondock-file-grants-") as temporary:
        corpus = build_corpus(Path(temporary))
        workspace = corpus.root / "workspace"
        anchor = workspace / "anchor"
        renamed = workspace / "renamed"
        outside = corpus.root / "outside"
        link = workspace / "current"
        first_target, next_target = workspace / "first-target", workspace / "next-target"
        state = corpus.root / "state"
        anchor.mkdir(parents=True)
        outside.mkdir()
        state.mkdir(mode=0o700)
        first_target.mkdir()
        next_target.mkdir()
        link.symlink_to(first_target, target_is_directory=True)
        for sid in ["granted-session", "ungranted-session"]:
            row = claude_row(sid, "user", "u0", None, f"See `{anchor}/` and `{link}/`.", cwd=str(workspace))
            corpus.put(sid, "claude", [row], [])
        uid, other = corpus.uid("granted-session"), corpus.uid("ungranted-session")
        kwargs = dict(state_dir=state, file_roots=(workspace,), file_write_roots=(workspace,))
        with isolated_server(corpus, args.binary, **kwargs) as (base, opener):
            status, body = request(opener, base, uid, str(link) + "/")
            assert status == 200 and body["path"] == str(first_target), body
            link.unlink()
            link.symlink_to(next_target, target_is_directory=True)
            status, body = request(opener, base, uid, str(link) + "/")
            assert status == 200 and body["path"] == str(next_target), body
            print("PASS grant: a fresh reference click follows a retargeted symlink", flush=True)
            status, body = request(opener, base, uid, str(anchor) + "/")
            assert status == 200, body
            status, body = request(opener, base, uid, str(anchor) + "/",
                                   action={"action": "rename", "paths": [str(anchor)], "name": "renamed"})
            assert status == 200 and renamed.is_dir() and not anchor.exists(), body
            status, body = request(opener, base, uid, str(anchor) + "/", path=outside)
            assert status == 200 and body["path"] == str(outside), body
            status, body = request(opener, base, uid, str(anchor) + "/", mode="jobs")
            assert status == 200, body
            print("PASS grant: original anchor renamed, navigation and jobs remain available", flush=True)
        with isolated_server(corpus, args.binary, **kwargs) as (base, opener):
            status, body = request(opener, base, uid, str(link) + "/")
            assert status == 200 and body["path"] == str(next_target), body
            status, body = request(opener, base, uid, str(anchor) + "/", path=renamed)
            assert status == 200, body
            status, body = request(opener, base, uid, str(anchor) + "/",
                                   action={"action": "new-file", "destination": str(outside), "name": "after-restart.txt"})
            assert status == 200 and (outside / "after-restart.txt").is_file(), body
            for rejected_uid, rejected_ref in [(other, str(anchor) + "/"), (uid, "/unmentioned"),
                                                ("claude:missing-grant-session", str(anchor) + "/")]:
                status, body = request(opener, base, rejected_uid, rejected_ref, path=outside)
                assert status in {400, 404, 501}, (status, body)
            status, body = request(opener, base, uid, str(anchor) + "/",
                                   action={"action": "delete", "paths": [str(renamed)]})
            assert status == 200 and not renamed.exists(), body
            status, body = request(opener, base, uid, str(anchor) + "/", mode="jobs")
            assert status == 200, body
            print("PASS restart: saved grant permits outside-root writes and survives deletion; other scopes rejected", flush=True)
        assert (state / "file-browser-grants.json").is_file()


if __name__ == "__main__":
    main()
