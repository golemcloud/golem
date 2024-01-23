#!/usr/bin/env bash
set -uo pipefail

script_full_path=$(dirname "$0")

cd "${script_full_path}"/../golem-cloud-server-oss || exit

cargo build
../target/debug/cloud-server-oss-yaml > ../openapi/golem-cloud-server-oss.yaml
