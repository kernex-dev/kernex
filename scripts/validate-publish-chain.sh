#!/usr/bin/env bash
#
# Validate that no publishable crate in the workspace depends on a workspace
# member with publish = false. The latter cannot ship to crates.io, so any
# publishable crate that imports it via path will fail `cargo publish` at
# package time with "no matching package named `<dep>` found" once the
# tarball strips the path component and tries to resolve from the registry.
#
# Run before tagging a release. Reads `cargo metadata --format-version 1
# --no-deps`, builds the set of `publish = false` member names, walks each
# publishable member's path-deps, and reports any edge that crosses from
# publishable to unpublishable.
#
# Exit codes:
#   0  no violations; safe to tag
#   1  one or more publishable crates depend on a publish = false member
#   2  cargo metadata failed or the workspace shape is unrecognised
#
# Background: this check was added after the v0.6.0 publish chain failed
# mid-flight because `kernex-runtime` (publishable) imported the then-
# `publish = false` `kernex-adapter-core`. See
# `.claude/docs/LEARNINGS.md` entry "v0.6.0 publish chain" for the full
# recovery story.

set -euo pipefail

cd "$(dirname "$0")/.."

if ! command -v cargo >/dev/null 2>&1; then
    echo "error: cargo not found on PATH" >&2
    exit 2
fi

if ! command -v python3 >/dev/null 2>&1; then
    echo "error: python3 not found on PATH" >&2
    exit 2
fi

if ! metadata="$(cargo metadata --format-version 1 --no-deps 2>&1)"; then
    echo "error: cargo metadata failed" >&2
    echo "$metadata" >&2
    exit 2
fi

python3 - "$metadata" <<'PY'
import json
import sys

md = json.loads(sys.argv[1])

# cargo metadata represents publish = false as an empty list []; anything
# else (including absent / None) means the crate is publishable.
unpublishable = {
    p["name"]
    for p in md["packages"]
    if p.get("publish") == []
}

violations = []
for p in md["packages"]:
    if p.get("publish") == []:
        continue
    name = p["name"]
    for dep in p.get("dependencies", []):
        if not dep.get("path"):
            continue
        if dep["name"] in unpublishable:
            violations.append((name, dep["name"]))

if violations:
    print("publish-chain VIOLATION:", file=sys.stderr)
    for crate, dep in violations:
        print(
            f"  {crate} (publishable) depends on path-member "
            f"{dep} (publish = false)",
            file=sys.stderr,
        )
    print(
        "\nFix: either promote the dep to `publish = true` and add it to "
        "the publish-crates.yml chain, or inline its symbols into the "
        "publishable crate, or remove the dependency.",
        file=sys.stderr,
    )
    sys.exit(1)

publishable_count = sum(
    1 for p in md["packages"] if p.get("publish") != []
)
print(
    f"publish-chain OK: {publishable_count} publishable members, "
    f"{len(unpublishable)} publish = false members, no crossover edges."
)
PY
