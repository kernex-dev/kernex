#!/usr/bin/env bash
# Publish a workspace crate unless its current workspace version already
# exists on crates.io. Makes the publish chain resumable: when a mid-chain
# step fails transiently (index download hiccup, network), re-dispatching
# the workflow skips every crate that already made it instead of halting
# on cargo publish's "already uploaded" error at the first step.
#
# Usage: publish-if-needed.sh <crate-name>
# Requires: cargo, jq, curl. CARGO_REGISTRY_TOKEN must be set for the
# actual publish (unused when the version is already live).
set -euo pipefail

crate="$1"

version=$(cargo metadata --format-version 1 --no-deps |
  jq -r ".packages[] | select(.name == \"$crate\") | .version")

if [ -z "$version" ] || [ "$version" = "null" ]; then
  echo "error: crate '$crate' not found in workspace metadata" >&2
  exit 1
fi

# crates.io rejects requests without a User-Agent (parses as NOT FOUND
# elsewhere); always send one. A 200 on the exact-version endpoint means
# the publish already happened.
if curl -fsS -A "kernex-publish-chain (support@kernex.dev)" \
  "https://crates.io/api/v1/crates/$crate/$version" >/dev/null 2>&1; then
  echo "$crate@$version already on crates.io; skipping publish"
  exit 0
fi

exec cargo publish -p "$crate" --locked
