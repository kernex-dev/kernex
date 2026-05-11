#!/usr/bin/env bash
# scripts/check-cold-start.sh <criterion-log-path> <threshold-ms>
#
# Parses the median time from a criterion bench log line of the form:
#
#   cold_start/memory_search_cold_start
#                           time:   [1.87 ms 1.90 ms 1.94 ms]
#
# and exits non-zero if the median exceeds <threshold-ms>.
#
# Criterion emits three numbers in the time array: lower bound, median,
# upper bound (from --sample-size and --measurement-time defaults). We
# enforce on the median.
#
# Used by .github/workflows/ci.yml to close FU-A-03 (promote cold_start
# from informational to a hard CI gate at the 50 ms threshold from
# V2-EXECUTION-PLAN decision 1.4).

set -euo pipefail

if [ "$#" -ne 2 ]; then
    echo "usage: $0 <criterion-log-path> <threshold-ms>" >&2
    exit 2
fi

log="$1"
threshold_ms="$2"

if [ ! -f "$log" ]; then
    echo "::error::criterion log not found: $log" >&2
    exit 1
fi

# Find the bench name + the following time line. awk picks the line that
# starts with `time:` after the matching bench name and pulls the three
# numbers. Criterion outputs units inline (ms or us or s); we normalize
# to milliseconds.
read -r unit lower median upper < <(awk '
    /^cold_start\/memory_search_cold_start$/ { found = 1; next }
    found && /time:/ {
        # Strip "[" and "]"; collapse whitespace.
        gsub(/\[|\]/, "")
        # The line looks like:  time:   1.87 ms 1.90 ms 1.94 ms
        # Tokens after "time:" are: <num1> <unit> <num2> <unit> <num3> <unit>
        # We trust criterion to emit the same unit for all three.
        print $3, $2, $4, $6
        exit
    }
' "$log")

if [ -z "${median:-}" ]; then
    echo "::error::could not parse criterion output for cold_start/memory_search_cold_start in $log" >&2
    cat "$log" >&2
    exit 1
fi

# Convert to milliseconds based on unit. Criterion uses "ns", "us"/"\xc2\xb5s", "ms", "s".
case "$unit" in
    ns)  factor=0.000001 ;;
    us)  factor=0.001 ;;
    µs)  factor=0.001 ;;
    ms)  factor=1 ;;
    s)   factor=1000 ;;
    *)   echo "::error::unknown criterion time unit: $unit" >&2; exit 1 ;;
esac

median_ms=$(awk -v m="$median" -v f="$factor" 'BEGIN { printf "%.3f", m * f }')

# Compare median_ms <= threshold_ms using awk (floating point).
over=$(awk -v m="$median_ms" -v t="$threshold_ms" 'BEGIN { print (m > t) ? "1" : "0" }')

if [ "$over" = "1" ]; then
    echo "::error::cold_start/memory_search_cold_start median ${median_ms} ms exceeds ${threshold_ms} ms threshold"
    echo "lower=${lower}${unit} median=${median}${unit} upper=${upper}${unit}"
    exit 1
fi

echo "cold_start/memory_search_cold_start: median ${median_ms} ms (lower=${lower}${unit} upper=${upper}${unit}) under ${threshold_ms} ms threshold"
