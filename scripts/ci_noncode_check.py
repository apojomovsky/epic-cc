#!/usr/bin/env python3
"""Shared CI classifier: can a change affect the build? Single source of
truth for the non-code allowlist (the _NON_CODE_RE below); anything not on
it counts as code-affecting, so a misclassified file only causes an extra
CI run, never a missed break (workflow files are deliberately not exempt).
Consumed by ci-discover-affected-modules.py (import) and family-check.yml
(CLI: prints "true" to skip the real-target steps, "false" to run full CI).
"""

import re
import subprocess
import sys

_NON_CODE_RE = re.compile(
    r"("
    r"\.md$"
    r"|(^|/)docs/"
    r"|^LICENSE"
    r"|\.gitignore$|\.gitattributes$|\.clang-format$|\.editorconfig$"
    r"|\.(svg|png|jpg|jpeg|gif|ico)$"
    r"|^scripts/bootstrap\.sh$"
    r"|^scripts/install-git-hooks\.sh$"
    r"|^Makefile$"
    r")"
)


def is_non_code(changed_files):
    """True iff every changed file is provably unable to affect the build."""
    return all(_NON_CODE_RE.search(p) for p in changed_files)


def main():
    base = sys.argv[1] if len(sys.argv) > 1 else ""
    if not base:
        sys.stderr.write("usage: ci_noncode_check.py <base-ref>\n")
        return 2
    changed = [
        line
        for line in subprocess.run(
            ["git", "diff", "--name-only", f"{base}...HEAD"],
            capture_output=True, text=True, check=True,
        ).stdout.splitlines()
        if line
    ]
    print("true" if is_non_code(changed) else "false")
    return 0


if __name__ == "__main__":
    sys.exit(main())
