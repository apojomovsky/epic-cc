"""Caller map for folded functions: who absorbed whom.

Compares the linked IR (`merged.ll` from `--save-temps`) against the
post-`opt` module (`merged_opt.ll`) and reports each vanished function
with exactly one direct caller::

    python3 scripts/inline-map.py merged.ll merged_opt.ll [tu.ll ...] > inline.json

`density-profile.py --inline-map inline.json` rolls those callees back
into their caller when ranking, so the triage in docs/43 does not have
to be redone by hand for every listing. Functions with zero or several
callers are skipped with a stderr note: nothing here guesses.

Caveat: clang itself (`-O1` per-TU) already folds small TU-local
single callers before any saved artifact exists (redraw_brightness
into redraw), so those never appear as defines anywhere and no
define-diff can recover them. Recovering that layer needs debug
line attribution, not name matching; what ships here covers the
`wholeprog_opt` layer plus any TU-local fold clang declined.
"""

import json
import re
import sys

DEFINE = re.compile(r"^define\b.*?@([A-Za-z_0-9.]+)\s*\(", re.MULTILINE)
CALL = re.compile(r"\b(?:call|invoke)\b[^@\n]*?@([A-Za-z_0-9.]+)\s*\(")


def defines(text):
    """Function names with a `define` body, in order."""
    return [
        m.group(1) for m in DEFINE.finditer(text) if not m.group(1).startswith("llvm.")
    ]


def callers_of(text):
    """Map callee to the set of functions with a direct call site."""
    callers = {}
    current = None
    for line in text.splitlines():
        m = DEFINE.match(line)
        if m:
            current = None if m.group(1).startswith("llvm.") else m.group(1)
            continue
        if current is None:
            continue
        for callee in CALL.findall(line):
            if not callee.startswith("llvm."):
                callers.setdefault(callee, set()).add(current)
    return callers


def tu_indexes(texts):
    """Map each function to the translation units defining it."""
    where = {}
    for index, text in enumerate(texts):
        for fn in defines(text):
            where.setdefault(fn, []).append(index)
    return where


def inline_map(pre_text, post_text, tu_texts=()):
    """{caller: [vanished single-caller callees]} plus skip notes.

    The vanilla pass compares one linked module against its optimized
    form, which sees only `wholeprog_opt` folds. Clang already folds
    TU-local single callers before the link, so with per-TU sources
    (`--save-temps` numbered outputs) those vanish from the linked
    module itself: a function defined in exactly one TU, gone from the
    post pass, with one caller in its own TU maps the same way.
    """
    pre = defines(pre_text)
    post = set(defines(post_text))
    callers = callers_of(pre_text)
    tu_of = tu_indexes(tu_texts)
    tu_callers = {}
    for index, text in enumerate(tu_texts):
        for callee, sites in callers_of(text).items():
            tu_callers.setdefault(callee, set()).update(sites)
    universe = pre if not tu_texts else [fn for fn in tu_of]
    clusters = {}
    skipped = []
    for fn in universe:
        if fn in post:
            continue
        sites = sorted(set(callers.get(fn, ())) | tu_callers.get(fn, set()))
        if len(sites) == 1:
            clusters.setdefault(sites[0], []).append(fn)
        else:
            skipped.append((fn, sites))
    return clusters, skipped


def main(argv):
    if len(argv) < 3:
        print(
            f"usage: {argv[0]} merged.ll merged_opt.ll [tu.ll ...]",
            file=sys.stderr,
        )
        return 2
    pre = open(argv[1]).read()
    post = open(argv[2]).read()
    tus = [open(path).read() for path in argv[3:]]
    clusters, skipped = inline_map(pre, post, tus)
    for fn, sites in skipped:
        print(f"skip {fn}: {len(sites)} callers {sites}", file=sys.stderr)
    json.dump(clusters, sys.stdout, indent=2, sort_keys=True)
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
