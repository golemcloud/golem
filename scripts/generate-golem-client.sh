#!/usr/bin/env bash
set -uo pipefail

script_full_path=$(dirname "$0")

client_version="${GOLEM_CLIENT_VERSION:-0.0.0-git}"

cd "${script_full_path}"/../golem-openapi-client-generator || exit

cargo build
./target/debug/golem-openapi-client-generator generate --spec-yaml ../openapi/golem-service.yaml --output-directory ../golem-client --name "golem-client" --client-version "${client_version}"
