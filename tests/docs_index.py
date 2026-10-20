#!/usr/bin/env python3
"""Generate a categorized navigation index for current docs/*.md contracts.

Prints the index to stdout. With --write, also writes docs/README.md as
the last step. The generated README is excluded from the index.
"""
# run_validation: skip
from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DOCS = ROOT / "docs"
OUT = DOCS / "README.md"
FENCE_RE = re.compile(r"^(\s{0,3})(`{3,}|~{3,})")
H1_RE = re.compile(r"^#\s+(.+?)\s*$")
LINK_RE = re.compile(r"!?\[([^\]]*)\]\([^)]*\)")
GROUPS = {
    "开发与交付": {"onboarding.md", "runbook-dev.md", "validation.md", "delegation.md", "adding-a-cli.md", "deployment.md", "release.md", "public-release.md", "deploy-hub.md", "deploy-macos.md", "deploy-windows.md", "replacement-checklist.md", "network-topology.md"},
    "SDK、对话与界面": {"sdk-ui.md", "native-sdk.md", "sdk-feature-audit.md", "codex-sdk.md", "grok-sdk.md", "agy-sdk.md", "claude-sdk.md", "opencode-sdk.md", "kiro-sdk.md", "conversation.md", "composer-input.md", "cli-state.md", "codex-commands.md", "quotas.md", "frontend-migration-surfaces.md", "question-card-regression-audit.md"},
    "会话、历史与文件": {"read-model.md", "history-pages.md", "native-input.md", "append-cache.md", "native-tags.md", "session-clone.md", "session-move.md", "trash.md", "metadata.md", "media.md", "media-pagination-design.md", "files.md", "external-links.md", "delivery-scope.md", "agy.md", "opencode.md", "kiro.md", "codex-names.md", "grok-metadata.md", "nesting-regression-audit.md"},
    "终端与生命周期": set(),
    "架构、接口与运行服务": {"architecture.md", "module-map.md", "environment.md", "error-codes.md", "route-ledger.md", "glossary.md", "capabilities.md", "hub.md", "diagnostics.md", "logging.md", "bug-report.md", "security-model.md", "processes.md", "process-links.md", "liveness.md"},
    "历史资料（按需查阅）": {"perf-methodology.md", "performance.md", "process-launch-audit.md", "history-incidents.md"},
}
ORDER = tuple(GROUPS)
INTRO = """# 文档索引

开发从 [AGENTS.md](../AGENTS.md)、[开发手册](runbook-dev.md) 与受影响合同开始。
当前行为写在专题合同，未完成工作只在 [TODO](../TODO.md)；交付结果、事故时间线和
完整提交目录由 Git 历史保存，不作为当前规范。历史资料仅在相关任务时查阅。

本索引只生成分类和标题，避免复制易过期的摘要、行数或功能状态。
重新生成：`python3 tests/docs_index.py --write`。
"""


def visible_lines(text: str):
    closing = None
    for line in text.splitlines():
        match = FENCE_RE.match(line)
        if match:
            marker = match.group(2)
            if closing is None:
                closing = (marker[0], len(marker))
                continue
            if marker[0] == closing[0] and len(marker) >= closing[1]:
                closing = None
                continue
        if closing is None:
            yield line


def plain(text: str) -> str:
    return LINK_RE.sub(r"\1", text)


def classify(name: str) -> str:
    for group, names in GROUPS.items():
        if name in names:
            return group
    if name.startswith(("terminal-", "lifecycle-", "host-")) or name == "session-host.md":
        return "终端与生命周期"
    return "架构、接口与运行服务"


def title_of(lines: list[str], fallback: str) -> str:
    for line in lines:
        match = H1_RE.match(line)
        if match:
            return plain(re.sub(r"\s+#+\s*$", "", match.group(1).strip()))
    return fallback




def cell(text: str) -> str:
    return text.replace("|", "\\|").replace("\n", " ")


def collect() -> list[dict]:
    rows = []
    for path in sorted(DOCS.glob("*.md")):
        if path.name == OUT.name:
            continue
        text = path.read_text(encoding="utf-8")
        vis = list(visible_lines(text))
        rows.append({
            "name": path.name,
            "title": title_of(vis, path.stem),
            "section": classify(path.name),
        })
    return rows


def render(rows: list[dict]) -> str:
    parts = [INTRO.rstrip(), ""]
    for section in ORDER:
        items = [row for row in rows if row["section"] == section]
        if not items:
            continue
        parts.append(f"## {section}")
        parts.append("")
        parts.append("| 文档 | 内容 |")
        parts.append("| --- | --- |")
        for row in items:
            doc = f"[{row['name']}]({row['name']})"
            parts.append("| " + " | ".join(cell(value) for value in (
                doc, row["title"],
            )) + " |")
        parts.append("")
    return "\n".join(parts).rstrip() + "\n"


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--write", action="store_true", help="write docs/README.md")
    args = parser.parse_args(argv)
    markdown = render(collect())
    try:
        sys.stdout.write(markdown)
    except BrokenPipeError:
        return 0
    if args.write:
        OUT.write_text(markdown, encoding="utf-8")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except BrokenPipeError:
        raise SystemExit(0)
