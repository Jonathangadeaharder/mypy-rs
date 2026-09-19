#!/usr/bin/env python3
"""#61 gap attribution: split a macOS `sample` call-tree by module.

`sample` (no root needed) stacks every thread at ~1 ms; the tree lines are

    <count> <symbol> (in <module>) + <off> [addr]

where <count> is inclusive of children. SELF samples = count - sum of the
immediate children's counts; self samples are bucketed by the `(in <module>)`
annotation. Interpreted Python frames are invisible to `sample` (they are
bytecode): they all land under PyEval_EvalFrameDefault (in Python), so the
split answers "Rust extension time vs CPython time vs rest" and names the
Rust-side symbols when the dylib's symbol table resolves them.

Guard: exits non-zero if the tree parses to zero total samples, if a
negative self count appears (a parse bug is worse than no measurement), or
if the file is missing.

Usage: misc/gap_attrib_sample_split.py FILE [FILE ...]
"""

from __future__ import annotations

import re
import sys
from collections import Counter

# Tree-line prefix: sample(1) draws the tree with +, !, |, : and spaces,
# e.g. "    + ! :  53601 main  (in python3.13) + 36  [0x...]"; the count is
# the first bare integer after that prefix.
LINE = re.compile(r"^([ :+!|]+)\s*(\d+)\s+(.+?)\s+\(in ([^)]+)\)")
THREAD = re.compile(r"^\s*(\d+)\s+Thread_")


def parse_tree(path: str) -> tuple[Counter[str], Counter[str], int]:
    frames: list[tuple[int, int, str, str]] = []
    thread_samples = 0
    with open(path) as f:
        for line in f:
            if line.startswith("Total number in stack"):
                break  # the tail re-lists frames; only the tree counts
            tm = THREAD.match(line)
            if tm:
                thread_samples += int(tm.group(1))
                continue
            m = LINE.match(line)
            if not m:
                continue
            indent = len(m.group(1)) // 2
            frames.append((indent, int(m.group(2)), m.group(3).strip(), m.group(4).strip()))

    # Pre-order walk: a frame's children are following frames with greater
    # indent, up to the next frame at <= its indent. Accumulate inclusive
    # counts into the parent's child-sum; self = count - child-sum.
    children_sum: dict[int, int] = {}
    stack: list[int] = []
    for i, (indent, _count, _sym, _mod) in enumerate(frames):
        while stack and frames[stack[-1]][0] >= indent:
            j = stack.pop()
            if stack:
                k = stack[-1]
                children_sum[k] = children_sum.get(k, 0) + frames[j][1]
        stack.append(i)
    while stack:  # drain: the deepest tail frames still owe their parents
        j = stack.pop()
        if stack:
            k = stack[-1]
            children_sum[k] = children_sum.get(k, 0) + frames[j][1]

    self_by_mod: Counter[str] = Counter()
    top_by_mod: Counter[str] = Counter()
    for i, (_indent, count, sym, mod) in enumerate(frames):
        self_n = count - children_sum.get(i, 0)
        if self_n < 0:
            raise SystemExit(f"negative self samples at frame {i} in {path}: {frames[i]}")
        if self_n > 0:
            self_by_mod[mod] += self_n
            top_by_mod[f"{mod}::{sym.split(' + ')[0]}"] += self_n
    return self_by_mod, top_by_mod, thread_samples


def main() -> int:
    if len(sys.argv) < 2:
        print(__doc__)
        return 2
    for path in sys.argv[1:]:
        self_by_mod, top_by_mod, thread_samples = parse_tree(path)
        if thread_samples == 0 and not self_by_mod:
            raise SystemExit(f"no samples parsed from {path}")
        print(f"== {path}")
        print(f"thread samples {thread_samples}  self-summed {sum(self_by_mod.values())}")
        for mod, n in self_by_mod.most_common(15):
            share = 100 * n / thread_samples if thread_samples else 0
            print(f"  {n:>8} {share:5.1f}%  in {mod}")
        print("  -- top symbols (self) --")
        for key, n in top_by_mod.most_common(30):
            print(f"  {n:>8}  {key}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
