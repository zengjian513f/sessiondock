#!/usr/bin/env python3
"""User-visible CJK/fullwidth text changed vs frozen reference/legacy-web.

Differing .js/.html/.css via legacy_asset_diff.classify. JS/CSS: quotes/templates
(comments skipped; ' and " cannot span lines); HTML: text nodes plus title,
placeholder, aria-label. Added/removed/changed (ratio ≥ 0.6); documented if a
12-char substring appears in docs/migration.md. Always exits 0.
"""
# run_validation: skip
import argparse, json, re, signal, sys
from difflib import SequenceMatcher
from html.parser import HTMLParser
from pathlib import Path
from legacy_asset_diff import LEGACY, REFERENCE, ROOT, classify, collect

SUFFIX = {".js", ".html", ".css"}
ATTRS = {"title", "placeholder", "aria-label"}
SKIP = {"script", "style"}
USER_RE = re.compile(r"[\u3000-\u30ff\u3400-\u4dbf\u4e00-\u9fff\uf900-\ufaff\uff00-\uffef\u2026]")
MIG = ROOT / "docs" / "migration.md"

def interesting(text):
    return bool(text) and USER_RE.search(text) is not None

def quoted(text):
    out, i, n, quote, start, block = [], 0, len(text), "", 0, False
    while i < n:
        c, two = text[i], text[i:i + 2]
        if block:
            block = two != "*/"
            i += 2 if two == "*/" else 1
            continue
        if quote:
            if c == "\\":
                i += 2
                continue
            if quote != "`" and c == "\n":
                quote = ""
                continue
            if c == quote:
                body = text[start + 1:i]
                if interesting(body):
                    out.append((text.count("\n", 0, start) + 1, body))
                quote = ""
            i += 1
            continue
        if two == "//":
            nl = text.find("\n", i)
            i = n if nl < 0 else nl
            continue
        if two == "/*":
            block, i = True, i + 2
            continue
        if c in "'\"`":
            quote, start = c, i
        i += 1
    return out


class HtmlHits(HTMLParser):
    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.hits, self.depth = [], 0

    def handle_starttag(self, tag, attrs):
        if tag in SKIP:
            self.depth += 1
            return
        if not self.depth:
            for key, val in attrs:
                if key in ATTRS and interesting(val):
                    self.hits.append((self.getpos()[0], val))

    def handle_endtag(self, tag):
        if tag in SKIP and self.depth:
            self.depth -= 1

    def handle_data(self, data):
        text = data.strip()
        if not self.depth and interesting(text):
            self.hits.append((self.getpos()[0], text))


def extract(path):
    if path is None or not path.is_file():
        return []
    text = path.read_text(encoding="utf-8", errors="replace")
    if path.suffix.lower() != ".html":
        return quoted(text)
    parser = HtmlHits()
    parser.feed(text)
    return parser.hits


def documented(text, blob):
    return len(text) >= 12 and any(text[i:i + 12] in blob for i in range(len(text) - 11))


def pair_off(rel, new, old, blob):
    used_n, used_o = [False] * len(new), [False] * len(old)
    for i, (_, nt) in enumerate(new):
        for j, (_, ot) in enumerate(old):
            if not used_o[j] and nt == ot:
                used_n[i] = used_o[j] = True
                break
    added = [(ln, t) for i, (ln, t) in enumerate(new) if not used_n[i]]
    removed = [(ln, t) for j, (ln, t) in enumerate(old) if not used_o[j]]
    ranks = sorted(((SequenceMatcher(None, a[1], r[1]).ratio(), i, j)
                    for i, a in enumerate(added) for j, r in enumerate(removed)), reverse=True)
    took_a, took_r, changed = set(), set(), []
    for rat, i, j in ranks:
        if rat < 0.6 or i in took_a or j in took_r:
            continue
        took_a.add(i); took_r.add(j)
        (aln, at), (rln, rt) = added[i], removed[j]
        changed.append({"file": rel, "from_line": rln, "from": rt, "to_line": aln, "to": at,
                        "ratio": round(rat, 3), "documented": documented(at, blob)})
    added_out = [{"file": rel, "line": ln, "text": t, "documented": documented(t, blob)}
                 for i, (ln, t) in enumerate(added) if i not in took_a]
    removed_out = [{"file": rel, "line": ln, "text": t}
                   for j, (ln, t) in enumerate(removed) if j not in took_r]
    return added_out, removed_out, changed


def emit_text(added, removed, changed):
    print(f"added {len(added)}  removed {len(removed)}  changed {len(changed)}")
    print("\n--- added ---")
    for row in added:
        flag = "documented" if row["documented"] else "undocumented"
        print(f"{row['file']}:{row['line']}\t{flag}\t{row['text']}")
    print("\n--- removed ---")
    for row in removed:
        print(f"{row['file']}:{row['line']}\t{row['text']}")
    print("\n--- changed ---")
    for row in changed:
        flag = "documented" if row["documented"] else "undocumented"
        print(f"{row['file']}:{row['from_line']}->{row['to_line']}\t{flag}\t"
              f"{row['ratio']}\t{row['from']}  =>  {row['to']}")


def main():
    signal.signal(signal.SIGPIPE, signal.SIG_DFL)
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args()
    blob = MIG.read_text(encoding="utf-8", errors="replace") if MIG.is_file() else ""
    added, removed, changed = [], [], []
    legacy_files, reference_files = collect(LEGACY), collect(REFERENCE)
    for row in classify(legacy_files, reference_files):
        rel = row["file"]
        if Path(rel).suffix.lower() not in SUFFIX or row["status"] == "identical":
            continue
        a, r, c = pair_off(rel, extract(legacy_files.get(rel)),
                           extract(reference_files.get(rel)), blob)
        added.extend(a)
        removed.extend(r)
        changed.extend(c)
    report = {"added": added, "removed": removed, "changed": changed}
    if args.json:
        json.dump(report, sys.stdout, ensure_ascii=False, indent=2)
        sys.stdout.write("\n")
    else:
        emit_text(added, removed, changed)
    return 0


if __name__ == "__main__":
    sys.exit(main())
