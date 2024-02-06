#!/usr/bin/env bash
set -uo pipefail

script_full_path=$(dirname "$0")

cd "${script_full_path}"/.. || exit

expected_version=$( ./scripts/expected-client-version.sh )
cargo build
./scripts/generate-openapi-yaml.sh
GOLEM_CLIENT_VERSION="${expected_version}" ./scripts/generate-golem-client.sh
mkdir -p ./golem-cli/.cargo
echo 'paths = ["../golem-client"]' > ./golem-cli/.cargo/config.toml
(cd golem-cli && cargo build)
