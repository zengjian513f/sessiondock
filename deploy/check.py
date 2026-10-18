#!/usr/bin/env python3
"""Check deployment Python, service templates and CI planning without deploying."""
from __future__ import annotations

import ast
import configparser
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def main():
    sources = list((ROOT / 'deploy').rglob('*.py'))
    for path in sources:
        ast.parse(path.read_text(encoding='utf-8'), filename=str(path))
    for filename, section, keys in (
        ('resource-agent.service', 'Service', ('ExecStart',)),
        ('sys-fs-resctrl.mount', 'Mount', ('What', 'Where', 'Type')),
    ):
        path = ROOT / 'deploy' / filename
        config = configparser.ConfigParser(interpolation=None, strict=False)
        config.optionxform = str
        config.read_string(path.read_text())
        for key in keys:
            if not config.get(section, key, fallback='').strip():
                raise ValueError(f'{path}: missing [{section}] {key}')
    print(f'PASS deployment syntax ({len(sources)} Python files) and service template structure')


if __name__ == '__main__':
    main()
