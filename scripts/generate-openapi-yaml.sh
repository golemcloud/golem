#!/usr/bin/env bash
set -uo pipefail

script_full_path=$(dirname "$0")

# Template service
pushd "${script_full_path}"/../golem-template-service || exit
cargo build
../target/debug/golem-template-service-yaml > ../openapi/golem-template-service.yaml

# Pop back to the original directory
popd || exit

pushd "${script_full_path}"/../golem-worker-service || exit
cargo build
../target/debug/golem-worker-service-yaml > ../openapi/golem-worker-service.yaml

# Pop back to the original directory when done
popd || exit

# Merge API specs
pushd "${script_full_path}"/../golem-openapi-client-generator || exit
cargo build
cargo run -- merge --spec-yaml ../openapi/golem-template-service.yaml ../openapi/golem-worker-service.yaml --output-yaml ../openapi/golem-service.yaml
