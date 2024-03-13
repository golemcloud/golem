#!/usr/bin/env bash
set -uo pipefail

script_full_path=$(dirname "$0")

cd "${script_full_path}"/.. || exit

./scripts/build-all.sh
GOLEM_DOCKER_SERVICES=true GOLEM_TEST_TEMPLATES="./test-templates" RUST_LOG=info cargo test --test integration
