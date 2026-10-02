#!/bin/sh
# Runs run.sh inside the compat image, on Linux with bash 5 and GNU tools.
#   run-in-docker.sh BRUSH_CHECKOUT COMPAT_WASM [-- harness arguments...]
set -eu
here=$(cd "$(dirname "$0")" && pwd)
brush=$(cd "$1" && pwd); wasm_dir=$(cd "$(dirname "$2")" && pwd); wasm=$(basename "$2"); shift 2
docker build -q -t golem-bash-compat:local "$here" >/dev/null
docker volume create golem-bash-compat-cargo >/dev/null; docker volume create golem-bash-compat-target >/dev/null
# /scratch, where the cases run, is a tmpfs rather than the container's overlay root: before Linux
# 6.8 overlayfs drops RWF_APPEND, which Wasmtime uses for `>>`, so appends landed at offset 0.
# `--init` lets a signal to this script (a timeout, Ctrl-C) end the run: `docker run` passes it
# into the container, where the suite would otherwise run as PID 1 and ignore it.
exec docker run --rm --init --tmpfs /scratch:rw,exec -v "$here:/compat:ro" -v "$brush:/brush" -v "$wasm_dir:/wasm:ro" \
  -v golem-bash-compat-cargo:/usr/local/cargo/registry -v golem-bash-compat-target:/target -e CARGO_TARGET_DIR=/target -e CARGO=cargo \
  golem-bash-compat:local sh -c 'mkdir -p /scratch && cd /scratch && TMPDIR=/scratch exec /compat/run.sh /brush "/wasm/$0" "$(command -v bash)" "$@"' "$wasm" "$@"
