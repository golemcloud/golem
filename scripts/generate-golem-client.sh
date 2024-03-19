#!/usr/bin/env bash
set -uo pipefail

script_full_path=$(dirname "$0")

client_version="${GOLEM_CLIENT_VERSION:-0.0.0-git}"

cargo install golem-openapi-client-generator@0.0.1
golem-openapi-client-generator generate --spec-yaml ${script_full_path}/../openapi/golem-service.yaml --output-directory ${script_full_path}/../golem-client --name "golem-client" --client-version "${client_version}"
