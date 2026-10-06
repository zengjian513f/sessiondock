"""Shared fixture helpers extracted from the former `lifecycle_http_suite.py` (removed 2026-10-06
with the non-browser suites); imported by browser suites."""
# run_validation: skip
from __future__ import annotations


SHELL = ('stty -echo 2>/dev/null; printf "RS_SHELL_READY\\n"; '
         'while IFS= read -r c; do case "$c" in quit) exit 0 ;; *) printf "RS_UNKNOWN\\n" ;; esac; done')
