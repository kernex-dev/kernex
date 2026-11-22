#!/usr/bin/env bash
# Package or publish a workspace crate unless its current workspace version
# already exists on crates.io. Makes the publish chain resumable: when a
# mid-chain step fails transiently (index download hiccup, network),
# re-dispatching the workflow skips every crate that already made it instead
# of halting on cargo publish's "already uploaded" error at the first step.
#
# Usage: publish-if-needed.sh package <crate-name>
#        publish-if-needed.sh publish <crate-name>
#
# `package` builds the crate the way a user will (cargo package verifies by
# compiling it and every dependency, build scripts included). It refuses to
# run with CARGO_REGISTRY_TOKEN set, so no dependency's build script can
# read the token. `publish` uploads with --no-verify: it compiles nothing,
# because `package` already did, and it is the only mode that holds the token.
#
# Requires: cargo, jq, curl.
set -euo pipefail

mode="${1:-}"
crate="${2:-}"

case "$mode" in
  package|publish) ;;
  *)
    echo "usage: $0 package|publish <crate-name>" >&2
    exit 2
    ;;
esac
if [ -z "$crate" ]; then
  echo "usage: $0 package|publish <crate-name>" >&2
  exit 2
fi

if [ "$mode" = "package" ] && [ -n "${CARGO_REGISTRY_TOKEN:-}" ]; then
  echo "error: refusing to package with CARGO_REGISTRY_TOKEN set: packaging runs build scripts" >&2
  exit 1
fi

version=$(cargo metadata --format-version 1 --no-deps |
  jq -r --arg crate "$crate" '.packages[] | select(.name == $crate) | .version')

if [ -z "$version" ] || [ "$version" = "null" ]; then
  echo "error: crate '$crate' not found in workspace metadata" >&2
  exit 1
fi

# crates.io rejects requests without a User-Agent (parses as NOT FOUND
# elsewhere); always send one. A 200 on the exact-version endpoint means
# the publish already happened.
if curl -fsS -A "kernex-publish-chain (support@kernex.dev)" \
  "https://crates.io/api/v1/crates/$crate/$version" >/dev/null 2>&1; then
  echo "$crate@$version already on crates.io; skipping $mode"
  exit 0
fi

if [ "$mode" = "package" ]; then
  exec cargo package -p "$crate" --locked
fi
exec cargo publish -p "$crate" --locked --no-verify
