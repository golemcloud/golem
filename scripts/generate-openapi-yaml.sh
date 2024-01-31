#!/usr/bin/env bash
set -uo pipefail

script_full_path=$(dirname "$0")

cd "${script_full_path}"/../golem-service || exit

cargo build
../target/debug/golem-service-yaml > ../openapi/golem-service.yaml
