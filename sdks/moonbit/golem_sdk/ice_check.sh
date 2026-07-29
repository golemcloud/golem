#!/usr/bin/env bash
# Isolated moonc check of the agent/guest package. Pass the .mbt files to check as args.
# Reproduces the ICE: "output_value: integer cannot be read back on 32-bit platform"
set -u
MOON="$HOME/.moon/bin/moonc"
STD="$HOME/.moon/lib/core/_build/wasm/release/bundle"
B="_build/wasm/debug/check"
"$MOON" check -error-format json \
  "$@" \
  -w -44 \
  -o "$B/gen/interface/golem/agent/guest/guest.mi" \
  -pkg golemcloud/golem_sdk/gen/interface/golem/agent/guest \
  -std-path "$STD" \
  -i "$B/agents/agents.mi:agents" \
  -i "$B/interface/golem/agent/common/common.mi:common" \
  -i "$STD/prelude/prelude.mi:prelude" \
  -i "$B/interface/golem/core/types/types.mi:types" \
  -pkg-sources "golemcloud/golem_sdk/gen/interface/golem/agent/guest:gen/interface/golem/agent/guest" \
  -target wasm -workspace-path . \
  -all-pkgs "$B/all_pkgs.json"
echo "EXIT=$?"
