#!/usr/bin/env bash
# Publish a workspace crate to crates.io only if the current workspace version
# isn't already there. Makes publish.yml idempotent: a partial release (e.g. a
# transient crates.io network blip mid-run) can be re-dispatched without choking
# on `cargo publish` erroring over the crates that already went up.
set -euo pipefail

crate="$1"
ver=$(grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)".*/\1/')

already=$(curl -s -A "mira-release (https://github.com/everruns/mira)" "https://crates.io/api/v1/crates/${crate}" \
  | python3 -c "import sys,json; print('${ver}' in {v['num'] for v in json.load(sys.stdin).get('versions',[])})" 2>/dev/null || echo False)

if [ "$already" = "True" ]; then
  echo "${crate} ${ver} already on crates.io — skipping"
else
  echo "Publishing ${crate} ${ver}"
  cargo publish -p "${crate}"
fi
