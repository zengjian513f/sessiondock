"""Shared fixture helpers extracted from the former `send_claude_real.py` (removed 2026-10-06
with the non-browser suites); imported by browser suites."""
# run_validation: skip
import json
import os
import shutil
import subprocess
from pathlib import Path

# Set by the importing suite (prompt_claude_real) before `logged_in()`.
CLAUDE = None


PROMPT = "Reply with the single word OK."


MODEL = "claude-haiku-4-5-20251001"


PROXY_KEYS = ("HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "NO_PROXY",
              "http_proxy", "https_proxy", "all_proxy", "no_proxy")


def passthrough():
    """The only inherited variables: PATH, LANG and the proxy settings."""
    env = {"PATH": os.environ.get("PATH", "/usr/bin:/bin"),
           "LANG": os.environ.get("LANG", "C.UTF-8")}
    env.update({key: os.environ[key] for key in PROXY_KEYS if key in os.environ})
    return env


def isolated_config(tmp):
    """A private CLAUDE_CONFIG_DIR that reuses the login read-only."""
    real = Path(os.environ.get("CLAUDE_CONFIG_DIR", Path.home() / ".claude"))
    creds = real / ".credentials.json"
    config = tmp / "claude-config"
    config.mkdir(mode=0o700)
    copied = False
    if creds.is_file():
        shutil.copy2(creds, config / ".credentials.json")
        copied = True
    # The top-level onboarding state, if present, avoids first-run prompts.
    top = Path.home() / ".claude.json"
    if top.is_file():
        shutil.copy2(top, config / ".claude.json")
    (config / "projects").mkdir(mode=0o700)
    return config, copied


def trust_project(config, area):
    """Pre-accept the folder trust dialog for the throwaway cwd in the isolated
    config copy (the TUI would otherwise block on it), like the monkey
    that answers the prompt with Enter. Never touches the real ~/.claude.json."""
    top = config / ".claude.json"
    try:
        document = json.loads(top.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        document = {}
    document["hasCompletedOnboarding"] = True
    projects = document.setdefault("projects", {})
    projects[str(area)] = {**projects.get(str(area), {}), "hasTrustDialogAccepted": True,
                           "allowedTools": [], "hasClaudeMdExternalIncludesApproved": False,
                           "hasClaudeMdExternalIncludesWarningShown": False}
    top.write_text(json.dumps(document, ensure_ascii=False, indent=2))
    top.chmod(0o600)


def logged_in(config, work, sid):
    """A trivial cheapest one-shot proves the isolated config is authenticated
    and, with a fixed --session-id, creates the session JSONL the TUI resumes.
    (Claude Code writes nothing for a fresh TUI session until its first input,
    so a brand-new session cannot be a resolvable send target yet.)"""
    try:
        out = subprocess.run(
            [str(CLAUDE), "-p", PROMPT, "--model", MODEL, "--effort", "low",
             "--session-id", sid],
            cwd=work, env={**passthrough(), "HOME": str(work), "CLAUDE_CONFIG_DIR": str(config)},
            stdin=subprocess.DEVNULL, capture_output=True, timeout=90)
    except (OSError, subprocess.TimeoutExpired) as error:
        return False, str(error)
    if out.returncode != 0:
        detail = (out.stderr.decode("utf-8", "replace").strip()
                  or out.stdout.decode("utf-8", "replace").strip() or "nonzero exit")
        return False, detail[:200]
    return True, out.stdout.decode("utf-8", "replace").strip()[:80]


def request(opener, base, method, route, body=None):
    from urllib.request import Request
    from urllib.error import HTTPError
    data = json.dumps(body).encode() if body is not None else None
    req = Request(base + route, data=data, method=method,
                  headers={"Content-Type": "application/json", "Host": "localhost"})
    try:
        with opener.open(req, timeout=15) as resp:
            return resp.status, json.loads(resp.read() or b"{}")
    except HTTPError as err:
        raw = err.read()
        return err.code, (json.loads(raw) if raw else {})
