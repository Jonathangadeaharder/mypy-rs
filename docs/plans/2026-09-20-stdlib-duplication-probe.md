# Skeleton stdlib-fact duplication probe (2026-09-20)

Part of #151 (the stdlib TypeInfo producer). The reader-fixed trigger for
that producer is the first point where hand-built stdlib facts are
duplicated across two independent slices. This record fixes the
measurement and reports the first reading, at commit `19cfa68cc`.

## What is measured

- **Hand-built stdlib facts.** The records in
  `crates/mypy-rs-skel/fixtures/skeleton_fixtures.json` that the checker
  cannot derive from the corpus itself: 13 TypeInfos (builtins primitives,
  `builtins.object/type/function`, `abc.ABCMeta`, `typing.*` protocol bases)
  plus the primitive name map. The closure list that selects them is
  hand-maintained in `misc/skeleton_codegen.py:51`.
- **Slices.** The feature families of the support manifest
  (`crates/mypy-rs-skel/testdata/manifest.json`): the arm id prefix before
  the first dot.
- **Reads.** Two mechanical notions, reported separately:
  1. source-text reads: a supported arm's file uses the primitive name
     that resolves to the fact (annotation resolution maps the name to the
     record, and a comparison result type does the same for `bool`); and
  2. code-path reads: the fact's fullname appears as a string literal
     anywhere in the linked Rust sources (`mypy-rs-skel/src`,
     `type_kernel/src`).

## First reading (at `19cfa68cc`)

| fact | source-text arms | families | literal references in Rust sources |
| --- | --- | --- | --- |
| builtins.int | 6 | 4 (assignments, functions, control_flow, expressions) | n/a |
| builtins.bool | 3 | 3 | n/a |
| builtins.float | 3 | 3 | n/a |
| builtins.str | 3 | 3 | n/a |
| builtins.object | 0 | 0 | 657 |
| builtins.function | 0 | 0 | 176 |
| builtins.type | 0 | 0 | 121 |
| abc.ABCMeta | 0 | 0 | 20 |
| typing.Sequence | 0 | 0 | 15 |
| typing.Iterable | 0 | 0 | 13 |
| typing.Container | 0 | 0 | 2 |
| typing.Reversible | 0 | 0 | 2 |
| typing.Collection | 0 | 0 | 0 |

**Verdict: the trigger has already fired.** All four primitives are read by
at least three supported arms in at least three families. The flat
hand-maintained closure list has already become the ad-hoc semantic
database the trigger is designed to detect, so #151 is unblocked the moment
the operators slice (#150) lands; the roadmap order does not change.

## Interpretation limits

- The source-text measure proves reads only for facts that pass through
  annotation resolution or a result type; the code-path measure is a
  literal-string scan, so runtime-constructed fullnames and MRO/protocol
  consultation show up as lower numbers than real traffic, not as zeros.
- `typing.Collection` has zero literal references in the linked sources
  and zero source-text witnesses. It may still be consulted through the
  MRO or protocol paths of the other `typing.*` records, but it is the
  first candidate for closure pruning; the producer should confirm with a
  resolver-hit trace before deleting it.
- Counts include `#[cfg(test)]` modules in both crates, so they are
  upper bounds for production traffic, not a split of it.

## Re-running

```bash
python3 misc/skeleton_duplication_probe.py            # text report
python3 misc/skeleton_duplication_probe.py --json     # machine form
```

Re-run after every manifest change: the trigger verdict is the bottom line,
and it must be re-derived, not assumed, as slices grow.
