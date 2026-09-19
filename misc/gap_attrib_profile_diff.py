#!/usr/bin/env python3
"""#61 gap attribution: diff two cProfile runs (kernel-on vs kernel-off).

Prints, per function:
  - ncalls delta (load-invariant: cProfile counts calls exactly),
  - tottime delta in the cProfile clock (distorted by per-call instrument
    overhead; used as a ranking, never as an instruction estimate),
grouped as DELTA classes: present-only-on, grew, shrank.

The on/off split is taken from argv[1] argv[2] (defaults
cprofile-on.prof / cprofile-off.prof).

Guard: exits non-zero if either profile is missing or empty (a silent
empty diff is worse than no diff).
"""

from __future__ import annotations

import pstats
import sys
from pathlib import Path


def main() -> int:
    on_path = sys.argv[1] if len(sys.argv) > 1 else "/private/tmp/gap-attrib/cprofile-on.prof"
    off_path = sys.argv[2] if len(sys.argv) > 2 else "/private/tmp/gap-attrib/cprofile-off.prof"
    if not Path(on_path).exists() or not Path(off_path).exists():
        print(f"missing profile: {on_path} or {off_path}", file=sys.stderr)
        return 2
    on = pstats.Stats(on_path)
    off = pstats.Stats(off_path)
    if not on.stats or not off.stats:
        print("empty profile", file=sys.stderr)
        return 2

    def rows(st: pstats.Stats) -> dict[tuple[str, str], tuple[int, float, float]]:
        return {
            (file, name): (val[1], val[2], val[3]) for (file, _line, name), val in st.stats.items()
        }

    r_on, r_off = rows(on), rows(off)
    total_on = sum(v[1] for v in r_on.values())
    total_off = sum(v[1] for v in r_off.values())
    print(f"functions on={len(r_on)} off={len(r_off)}")
    print(
        f"cProfile tottime total on={total_on:.1f}s off={total_off:.1f}s "
        f"delta={total_on - total_off:+.1f}s"
    )

    print("\n== present only on-arm (top 40 by tottime) ==")
    only = sorted(((k, v) for k, v in r_on.items() if k not in r_off), key=lambda kv: -kv[1][1])
    for (file, name), (nc, tt, ct) in only[:40]:
        print(f"{tt:9.3f}s ct={ct:9.3f}s nc={nc:>10}  {file}:{name}")

    print("\n== grew on-arm (top 60 by tottime delta) ==")
    grew = sorted(
        ((k, r_on[k], r_off[k]) for k in r_on if k in r_off and r_on[k][1] - r_off[k][1] > 0),
        key=lambda kv: -(kv[1][1] - kv[2][1]),
    )
    for (file, name), (nc_on, tt_on, ct_on), (nc_off, tt_off, ct_off) in grew[:60]:
        print(
            f"dtt={tt_on - tt_off:+9.3f}s tt_on={tt_on:8.3f} tt_off={tt_off:8.3f} "
            f"nc={nc_on:>10}/{nc_off:>10} dnc={nc_on - nc_off:>+9}  {file}:{name}"
        )

    print("\n== shrank on-arm (top 20) ==")
    shrank = sorted(
        ((k, r_on[k], r_off[k]) for k in r_on if k in r_off and r_off[k][1] - r_on[k][1] > 0),
        key=lambda kv: -(kv[2][1] - kv[1][1]),
    )
    for (file, name), (nc_on, tt_on, ct_on), (nc_off, tt_off, ct_off) in shrank[:20]:
        print(
            f"dtt={tt_on - tt_off:+9.3f}s tt_on={tt_on:8.3f} tt_off={tt_off:8.3f} "
            f"nc={nc_on:>10}/{nc_off:>10}  {file}:{name}"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
