#!/usr/bin/env bash

set -euo pipefail

sdk_root="$(cd "$(dirname "$0")/.." && pwd)"
tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/check-wit-bindgen-version.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

cat >"$tmp_dir/wit-bindgen" <<'EOF'
#!/usr/bin/env bash
if [[ "${1:-}" == "--version" ]]; then
  # This is the stock release, not the required Golem fork.
  echo 'wit-bindgen-cli 0.59.0'
  exit 0
fi
touch "$WIT_BINDGEN_WAS_INVOKED"
exit 42
EOF
chmod +x "$tmp_dir/wit-bindgen"

export WIT_BINDGEN_WAS_INVOKED="$tmp_dir/invoked"
if PATH="$tmp_dir:$PATH" "$sdk_root/scripts/regen-bindings.sh" >/dev/null 2>&1; then
  echo "expected regeneration to reject stock wit-bindgen 0.59.0" >&2
  exit 1
fi

if [[ -e "$WIT_BINDGEN_WAS_INVOKED" ]]; then
  echo "stock wit-bindgen 0.59.0 passed the fork validation and was invoked" >&2
  exit 1
fi
