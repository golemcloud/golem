#!/bin/sh
# The slow conformance job: Brush's compatibility suite against the tool's shell, compared with the
# recorded baseline, and every matrix case, sweep tier included, checked against its golden.
# Re-checking the goldens against a live oracle (`stale`) is a SEPARATE job on its own schedule,
# not run here: it only needs to run when the oracle Dockerfile, the goldens or conformance.py
# change, or on a fixed schedule, not on every push -- see "Oracle drift" in the workflow.
#
#   ci.sh TARGET_DIR
#
# TARGET_DIR is the Cargo target directory to build the examples in. The Brush checkout the
# suite comes from is the fork at the revision the bash workspace pins.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
bash_dir=$(cd "$here/../.." && pwd)
target=$1

rev=$(sed -n 's/^brush-core = { git = "https:\/\/github.com\/Aditya1404Sal\/brush", rev = "\([0-9a-f]*\)" }$/\1/p' "$bash_dir/Cargo.toml")
[ -n "$rev" ] || { echo "ci.sh: no pinned Brush revision in $bash_dir/Cargo.toml" >&2; exit 1; }
brush=$(mktemp -d)
trap 'rm -rf "$brush"' EXIT
git -C "$brush" init -q
git -C "$brush" fetch -q --depth 1 https://github.com/Aditya1404Sal/brush "$rev"
git -C "$brush" checkout -q FETCH_HEAD

cargo build --manifest-path "$bash_dir/Cargo.toml" --locked --target-dir "$target" \
  -p bash-shell --example compat --example cooperative --features test-support --release \
  --target wasm32-wasip2
out=$(mktemp)
# The suite takes a few minutes. A case that never ends would otherwise hold the job until its own
# timeout; the harness has no per-case limit, so a shell that loops stalls the whole run.
limit=""
command -v timeout >/dev/null && limit="timeout 20m" # GNU coreutils; macOS has none
status=0
$limit "$here/run-in-docker.sh" "$brush" "$target/wasm32-wasip2/release/examples/compat.wasm" \
  >"$out" 2>&1 || status=$?
if [ "$status" -eq 124 ]; then
  echo "ci.sh: Brush's compat suite did not finish in 20 minutes: one of its cases hangs the shell" >&2
  exit 1
fi
python3 "$here/baseline.py" check "$out"

# Every case against its golden, including the sweep-tier corpora the per-PR job skips. The cases
# still known to differ are listed in sweep-known-failures.json: the job fails on a new failure,
# and on a listed case that now passes, so the list only shrinks.
python3 "$bash_dir/conformance/conformance.py" check --tier all \
  --wasm "$target/wasm32-wasip2/release/examples/cooperative.wasm" \
  --known-failures "$bash_dir/conformance/sweep-known-failures.json"
