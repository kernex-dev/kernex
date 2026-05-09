#!/usr/bin/env bash
# scripts/check-size.sh <variant-name> <max-bytes> <binary-path>
#
# Used by the size-gate CI workflow to enforce per-variant binary ceilings.
# Cross-platform: handles macOS (`stat -f%z`) and Linux (`stat -c%s`).

set -euo pipefail

if [ "$#" -ne 3 ]; then
    echo "usage: $0 <variant-name> <max-bytes> <binary-path>" >&2
    exit 2
fi

variant="$1"
max="$2"
bin="$3"

if [ ! -f "$bin" ]; then
    echo "::error::binary not found: $bin" >&2
    exit 1
fi

case "$(uname -s)" in
    Darwin)  size=$(stat -f%z "$bin") ;;
    Linux)   size=$(stat -c%s "$bin") ;;
    *)       echo "::error::unsupported platform: $(uname -s)" >&2; exit 1 ;;
esac

human() {
    awk -v n="$1" 'BEGIN{
        if (n>=1048576) printf "%.2f MB", n/1048576
        else if (n>=1024) printf "%.2f KB", n/1024
        else printf "%d B", n
    }'
}

echo "[$variant] $bin: $(human "$size") (limit: $(human "$max"))"

if [ "$size" -gt "$max" ]; then
    echo "::error::$variant binary exceeds ceiling ($(human "$size") > $(human "$max"))" >&2
    exit 1
fi
