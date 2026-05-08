#!/bin/sh
# Thin wrapper — the actual logic is in scripts/release/main.mbt
# Run: ./release.sh 0.1.0  or  ./release.sh --dev
cd "$(dirname "$0")/scripts" && exec moon run release -- "$@"
