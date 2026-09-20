#!/usr/bin/env python3
"""Duplication probe for the skeleton's hand-built stdlib facts (#151).

The reader-fixed trigger for building the stdlib TypeInfo producer is the
first point where hand-built stdlib facts are duplicated across two
independent slices. This probe measures the duplication mechanically:

- the stdlib facts the fixture file carries (the closure the checker
  cannot derive), one row per fact;
- for each fact, the supported manifest arms whose source text reads the
  primitive name that resolves to it (via the fixture `builtins_symbols`
  or the semanal name map), grouped by feature family.

A fact read by arms in two or more families has triggered the producer.
The probe is a report, not a gate: it always exits 0 on success.

Limits, stated so the numbers are not over-read:
- Source-text usage proves the read for names that pass through
  annotation resolution or a comparison result type. Facts consumed only
  through the kernel's subtype/protocol path (typing.*, abc.ABCMeta,
  builtins.object/type/function) have no source-text witness and are
  reported as `code-path` rows with a call-site count, not a slice count.
- Family is the arm id prefix before the first dot, the unit the reader
  names as a slice.

Usage: python3 misc/skeleton_duplication_probe.py [--json]
"""

from __future__ import annotations

import json
import os
import re
import sys
from collections import defaultdict

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SKEL = os.path.join(REPO_ROOT, "crates", "mypy-rs-skel")
MANIFEST = os.path.join(SKEL, "testdata", "manifest.json")
FIXTURES = os.path.join(SKEL, "fixtures", "skeleton_fixtures.json")
SOURCE_ROOTS = (
    os.path.join(SKEL, "src"),
    os.path.join(REPO_ROOT, "crates", "type_kernel", "src"),
)

PRIMITIVES = {
    "int": "builtins.int",
    "bool": "builtins.bool",
    "str": "builtins.str",
    "float": "builtins.float",
}


def load_facts() -> tuple[list[str], dict[str, str]]:
    with open(FIXTURES, encoding="utf-8") as handle:
        data = json.load(handle)
    facts = sorted(record["fullname"] for record in data["typeinfos"])
    symbols = dict(data["builtins_symbols"])
    return facts, symbols


def arm_rows() -> list[tuple[str, str, list[str]]]:
    with open(MANIFEST, encoding="utf-8") as handle:
        manifest = json.load(handle)
    rows = []
    for capability in manifest["capabilities"]:
        if capability.get("status") != "supported":
            continue
        arm = capability.get("arm")
        if not arm:
            continue
        path = os.path.join(SKEL, "testdata", arm)
        if not os.path.exists(path):
            continue
        with open(path, encoding="utf-8") as handle:
            source = handle.read()
        used = sorted(
            fullname
            for name, fullname in PRIMITIVES.items()
            if re.search(r"(?<![\w.])" + re.escape(name) + r"(?![\w])", source)
        )
        rows.append((capability["id"], arm, used))
    return rows


def code_path_counts(facts: list[str]) -> dict[str, int]:
    texts = []
    for root in SOURCE_ROOTS:
        for dirpath, _dirs, files in os.walk(root):
            for name in sorted(files):
                if name.endswith(".rs"):
                    path = os.path.join(dirpath, name)
                    with open(path, encoding="utf-8") as handle:
                        texts.append(handle.read())
    counts = {}
    for fact in facts:
        if fact in PRIMITIVES.values():
            continue
        counts[fact] = sum(
            len(re.findall(re.escape('"' + fact + '"'), text)) for text in texts
        )
    return counts


def build_report() -> dict:
    facts, symbols = load_facts()
    rows = arm_rows()
    by_fact: dict[str, list[str]] = defaultdict(list)
    for arm_id, _arm, used in rows:
        for fullname in used:
            by_fact[fullname].append(arm_id)
    families: dict[str, set[str]] = {}
    for fullname, arms in by_fact.items():
        families[fullname] = {arm.split(".")[0] for arm in arms}
    triggered = sorted(
        fact
        for fact, family_set in families.items()
        if len(by_fact[fact]) >= 2 and len(family_set) >= 2
    )
    return {
        "facts": facts,
        "symbols": symbols,
        "arms": [
            {"id": arm_id, "arm": arm, "facts": used} for arm_id, arm, used in rows
        ],
        "by_fact": {fact: by_fact[fact] for fact in sorted(by_fact)},
        "families": {fact: sorted(families[fact]) for fact in sorted(families)},
        "code_path_counts": code_path_counts(facts),
        "duplicated_facts": triggered,
    }


def render(report: dict) -> str:
    lines = []
    lines.append(f"fixture stdlib facts: {len(report['facts'])}")
    for fact in report["facts"]:
        lines.append(f"  {fact}")
    lines.append("")
    lines.append("supported arm -> primitive facts read from source text")
    for row in report["arms"]:
        lines.append(f"  {row['id']:45s} {', '.join(row['facts']) or '-'}")
    lines.append("")
    lines.append("fact -> arms (families)")
    for fact, arms in report["by_fact"].items():
        fams = report["families"][fact]
        lines.append(f"  {fact:22s} {len(arms)} arms, {len(fams)} families: {', '.join(arms)}")
    lines.append("")
    lines.append("facts with no source-text witness -> references in linked Rust sources")
    for fact, count in report["code_path_counts"].items():
        lines.append(f"  {fact:22s} {count}")
    lines.append("")
    verdict = report["duplicated_facts"]
    if verdict:
        lines.append(
            "TRIGGER FIRED: stdlib facts duplicated across independent slices: "
            + ", ".join(verdict)
        )
    else:
        lines.append("trigger not fired: no stdlib fact spans two or more families")
    return "\n".join(lines)


def main() -> int:
    report = build_report()
    if "--json" in sys.argv:
        print(json.dumps(report, indent=2, sort_keys=True))
    else:
        print(render(report))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
