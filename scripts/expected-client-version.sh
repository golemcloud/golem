#!/bin/bash

script_full_path=$(dirname "$0")

grep golem-client "${script_full_path}"/../golem-cli/Cargo.toml | sed -e 's/golem-client *= *"\([^"]*\)".*/\1/'
