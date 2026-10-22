#!/usr/bin/env python3
"""Rewrites the `version` in Cargo.toml's [package] and [workspace.package] tables.

Used by release-stamped.sh so the release tag is the only place a version
lives; the edit is textual (not a TOML round trip) to keep comments intact.
"""

import re
import sys

STAMPED_TABLES = {"[package]", "[workspace.package]"}
VERSION_LINE = re.compile(r'^(version\s*=\s*)"[^"]*"')


def stamp(text: str, version: str) -> tuple[str, int]:
    table = ""
    count = 0
    out = []
    for line in text.splitlines(keepends=True):
        header = line.strip()
        if header.startswith("["):
            table = header.split("#")[0].strip()
        elif table in STAMPED_TABLES and (match := VERSION_LINE.match(line)):
            line = f'{match.group(1)}"{version}"' + line[match.end() :]
            count += 1
        out.append(line)
    return "".join(out), count


def main() -> int:
    if len(sys.argv) != 3:
        print("usage: release-stamp-version.py <Cargo.toml> <version>", file=sys.stderr)
        return 2
    path, version = sys.argv[1:]
    with open(path, encoding="utf-8") as handle:
        text, count = stamp(handle.read(), version)
    if count == 0:
        print(f"{path}: no literal `version = \"...\"` in [package] or [workspace.package]", file=sys.stderr)
        return 1
    with open(path, "w", encoding="utf-8") as handle:
        handle.write(text)
    return 0


if __name__ == "__main__":
    sys.exit(main())
