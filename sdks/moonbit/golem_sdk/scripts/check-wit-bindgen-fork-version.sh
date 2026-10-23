#!/usr/bin/env bash

set -euo pipefail

sdk_root="$(cd "$(dirname "$0")/.." && pwd)"
tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/check-wit-bindgen-fork-version.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

cat >"$tmp_dir/wit-bindgen" <<'EOF'
#!/usr/bin/env bash
if [[ "${1:-}" == "--version" ]]; then
  # A Git build from upstream has the same version shape as the Golem fork.
  echo 'wit-bindgen-cli 0.59.0 (abcdef123 2026-09-30)'
  exit 0
fi
touch "$WIT_BINDGEN_WAS_INVOKED"
exit 42
EOF
chmod +x "$tmp_dir/wit-bindgen"

export WIT_BINDGEN_WAS_INVOKED="$tmp_dir/invoked"
PATH="$tmp_dir:$PATH" "$sdk_root/scripts/regen-bindings.sh" >/dev/null 2>&1 || true

if [[ -e "$WIT_BINDGEN_WAS_INVOKED" ]]; then
  echo "non-Golem Git build passed the fork validation and was invoked" >&2
  exit 1
fi

mkdir -p "$tmp_dir/sdk/scripts"
cp "$sdk_root/scripts/install-wit-bindgen.sh" "$tmp_dir/sdk/scripts/"
cat >"$tmp_dir/cargo" <<'EOF'
#!/usr/bin/env bash
# Keep the installed generator in place to exercise the cache-hit version check.
exit 0
EOF
chmod +x "$tmp_dir/cargo"

if PATH="$tmp_dir:$PATH" WIT_BINDGEN_REV=0123456789abcdef0123456789abcdef01234567 \
  "$tmp_dir/sdk/scripts/install-wit-bindgen.sh" >/dev/null 2>&1; then
  echo "installation accepted a generator from a different source revision" >&2
  exit 1
fi
