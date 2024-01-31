#!/usr/bin/env bash
set -uo pipefail

script_full_path=$(dirname "$0")

cd "${script_full_path}"/../golem-openapi-client-generator || exit

cargo build
./target/debug/golem-openapi-client-generator --spec-yaml ../openapi/golem-service.yaml --output-directory ../golem-client --name "golem-client" --client-version "0.0.0-git"
