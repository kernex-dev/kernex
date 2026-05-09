#!/usr/bin/env python3
"""scripts/diff-bloat.py <baseline.txt> <current.txt>

Soft-warns if any crate's contribution grew more than 10% relative to the
committed baseline. Advisory only: always exits 0.

Parses output of `cargo bloat --release --crates -n N`. Each data line is:
    [pct1]% [pct2]% [size][unit] [crate]
where unit is one of B, KiB, MiB, GiB.
"""
from __future__ import annotations

import re
import sys
from pathlib import Path


GROWTH_THRESHOLD = 0.10  # 10%

LINE_RE = re.compile(
    r"^\s*([\d.]+)%\s+([\d.]+)%\s+([\d.]+)([A-Za-z]+)\s+(.+?)\s*$"
)
UNITS = {"B": 1, "KiB": 1024, "MiB": 1024 ** 2, "GiB": 1024 ** 3}


def parse(path: Path) -> dict[str, int]:
    """Return {crate: size_bytes}."""
    sizes: dict[str, int] = {}
    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        # Skip header, footer, "And N more crates" rollup, summary line.
        if not line.strip() or line.lstrip().startswith("File"):
            continue
        m = LINE_RE.match(line)
        if not m:
            continue
        size_num, unit, crate = m.group(3), m.group(4), m.group(5).strip()
        if unit not in UNITS:
            continue
        if crate.startswith("And ") or crate.startswith("."):
            continue
        sizes[crate] = int(float(size_num) * UNITS[unit])
    return sizes


def main(argv: list[str]) -> int:
    if len(argv) != 3:
        print("usage: diff-bloat.py <baseline.txt> <current.txt>", file=sys.stderr)
        return 2

    baseline_path = Path(argv[1])
    current_path = Path(argv[2])

    if not baseline_path.exists():
        print(f"baseline not found: {baseline_path}; skipping", file=sys.stderr)
        return 0
    if not current_path.exists():
        print(f"current not found: {current_path}; skipping", file=sys.stderr)
        return 0

    baseline = parse(baseline_path)
    current = parse(current_path)

    flagged = []
    new_crates = []
    for crate, cur_size in current.items():
        base_size = baseline.get(crate)
        if base_size is None:
            new_crates.append((crate, cur_size))
            continue
        if base_size == 0:
            continue
        growth = (cur_size - base_size) / base_size
        if growth > GROWTH_THRESHOLD:
            flagged.append((crate, base_size, cur_size, growth))

    if flagged:
        print(f"::warning::{len(flagged)} crate(s) grew > {int(GROWTH_THRESHOLD * 100)}% vs baseline:")
        for crate, base, cur, growth in sorted(flagged, key=lambda x: -x[3]):
            print(f"  {crate}: {base} -> {cur} bytes (+{growth * 100:.1f}%)")
    else:
        print(f"No crate grew more than {int(GROWTH_THRESHOLD * 100)}% vs baseline.")

    if new_crates:
        print(f"::notice::{len(new_crates)} new crate(s) appeared since baseline (informational):")
        for crate, size in sorted(new_crates, key=lambda x: -x[1])[:10]:
            print(f"  {crate}: {size} bytes")

    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
