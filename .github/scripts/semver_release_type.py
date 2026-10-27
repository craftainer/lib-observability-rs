#!/usr/bin/env python3
"""Pick the ``cargo semver-checks`` release type for a CI run.

Prints one line for ``$GITHUB_ENV``: ``SEMVER_RELEASE_TYPE=major`` when the
change is marked breaking with the Conventional Commits ``!`` marker (for
example ``feat(bulk)!: ...`` or ``fix!: ...``), else ``SEMVER_RELEASE_TYPE=``
(empty, so the ``cargo-semver-checks`` hook falls back to its ``minor``
default).

Which text is judged depends on the event: a ``pull_request`` uses the PR
title, a ``push`` to ``main`` uses the subject line of the head commit (a
squash merge carries the PR title there), anything else is left alone.

The title and commit message are attacker-controlled, so they arrive only
through the environment (``EVENT_NAME``, ``REF``, ``PR_TITLE``,
``COMMIT_MESSAGE``) and are matched here, never interpolated into a shell
script. Only the fixed strings above are ever printed.
"""

from __future__ import annotations

import os
import re

BREAKING_RE = re.compile(r"^[a-z]+(\([^)]*\))?!:")


def is_breaking(subject: str) -> bool:
    """Return True if a Conventional Commits subject carries the ``!`` marker."""
    return BREAKING_RE.match(subject) is not None


def release_type(event: str, ref: str, title: str, message: str) -> str:
    """Return ``major`` or the empty string for the given event context."""
    if event == "pull_request":
        subject = title
    elif event == "push" and ref == "refs/heads/main":
        subject = message.split("\n", 1)[0]
    else:
        return ""
    return "major" if is_breaking(subject.strip()) else ""


def main() -> int:
    """Print the ``$GITHUB_ENV`` line for the current environment."""
    env = os.environ
    value = release_type(
        env.get("EVENT_NAME", ""),
        env.get("REF", ""),
        env.get("PR_TITLE", ""),
        env.get("COMMIT_MESSAGE", ""),
    )
    print(f"SEMVER_RELEASE_TYPE={value}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
