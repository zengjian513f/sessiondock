"""Shared fixture helpers extracted from the former `provider_parity.py` (removed 2026-10-06
with the non-browser suites); imported by browser suites."""
# run_validation: skip
from __future__ import annotations
import json
from urllib.parse import quote
from urllib.request import HTTPRedirectHandler


MAX_RESPONSE_BYTES = 2 * 1024 * 1024


class NoRedirects(HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        raise RuntimeError("redirect refused: only the explicit fixture server is in scope")


def api(opener, base: str, uid: str, query: str = "") -> dict:
    # UID is computed only from the three hardcoded fixture paths, never taken
    # from a server inventory or supplied as an arbitrary user argument.
    url = base + "/api/messages/" + quote(uid, safe=":") + ("?" + query if query else "")
    with opener.open(url, timeout=10) as response:
        raw = response.read(MAX_RESPONSE_BYTES + 1)
        if len(raw) > MAX_RESPONSE_BYTES:
            raise RuntimeError("fixture response exceeds the allowed size")
        if response.status != 200:
            raise RuntimeError(f"fixture endpoint returned HTTP {response.status}")
        value = json.loads(raw)
    if not isinstance(value, dict) or value.get("meta", {}).get("uid") != uid:
        raise RuntimeError("server did not return the exact requested synthetic fixture")
    return value
