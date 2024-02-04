#!/usr/bin/env bash
set -uo pipefail

script_full_path=$(dirname "$0")

cd "${script_full_path}"/.. || exit

./scripts/build-all.sh
cd golem-cli
RUST_LOG=info cargo test
