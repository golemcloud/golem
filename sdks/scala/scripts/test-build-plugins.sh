#!/usr/bin/env bash
# Tests golemBuildComponent of the sbt and Mill plugins on fixtures that both builds link:
# - `singleModuleFixture` ships the fullLinkJS module, and sbt and Mill ship the same bytes;
# - `dynamicImportFixture` splits into several modules (js.dynamicImport) and must fail the build.
# Both use custom .sourcemap names so the plugins cannot identify source maps by a .map suffix.
# Both fixtures live under test-build-plugins/; sbt/build.sbt and mill/build.mill define them.
#
# Needs sbt, Mill, and the guest runtime WASMs from scripts/generate-agent-guest-wasm.sh.
# Every check runs and reports on its own; the script fails at the end if any check failed.
set -uo pipefail
cd "$(dirname "$0")/.."

out="$(mktemp -d)"
trap 'rm -rf "$out"' EXIT
failures=()
fail() {
  failures+=("$1")
  echo "FAIL: $1" >&2
}

sbt_run() { sbt -batch -no-colors -Dsbt.supershell=false "$@"; }
mill_run() { (cd mill && mill --no-daemon --ticker false "$@"); }

describe() { echo "$(wc -c <"$1") bytes, sha256 $(sha256sum "$1" | cut -d' ' -f1)"; }

# Checks that the bundle equals the fullLinkJS module and differs from the fastLinkJS module.
expect_full_link() {
  local tool=$1 bundle=$2 full=$3 fast=$4
  if [[ ! -f "$bundle" ]]; then
    fail "$tool: golemBuildComponent wrote no bundle for the single module fixture"
    return
  fi
  if [[ ! -f "$full" ]]; then
    fail "$tool: the fullLinkJS module $full is missing"
  elif ! cmp -s "$bundle" "$full"; then
    fail "$tool: the bundle ($(describe "$bundle")) is not the fullLinkJS module $full"
  fi
  if [[ ! -f "$fast" ]]; then
    fail "$tool: the fastLinkJS module $fast is missing"
  elif cmp -s "$bundle" "$fast"; then
    fail "$tool: the bundle equals the fastLinkJS module $fast"
  fi
}

# Checks that a split linker output fails golemBuildComponent with an error naming every module.
expect_split_rejected() {
  local tool=$1 status=$2 log=$3 linker_dir=$4 bundle=$5
  local modules
  modules=$(find "$linker_dir" -maxdepth 1 -type f -name '*.js' -printf '%f\n' 2>/dev/null | sort)
  if (($(grep -c . <<<"$modules") < 2)); then
    fail "$tool: the dynamic import fixture did not link into several modules in $linker_dir"
  fi
  ((status != 0)) || fail "$tool: golemBuildComponent succeeded on split linker output"
  [[ ! -e "$bundle" ]] || fail "$tool: golemBuildComponent wrote a bundle from split linker output"
  grep -q "needs a single module" <<<"$log" ||
    fail "$tool: the error does not say that a Golem component needs a single module"
  local module
  for module in $modules; do
    grep -qF "$module" <<<"$log" || fail "$tool: the error does not name the module $module"
  done
}

echo "==> sbt: single module fixture"
sbt_run "singleModuleFixture/fastLinkJS" "singleModuleFixture/golemBuildComponent single $out/sbt.js" ||
  fail "sbt: golemBuildComponent failed on the single module fixture"
sbt_target=test-build-plugins/single-module/target/scala-3.3.7
expect_full_link sbt "$out/sbt.js" "$sbt_target/singlemodulefixture-opt/main.js" \
  "$sbt_target/singlemodulefixture-fastopt/main.js"

echo "==> Mill: single module fixture"
mill_run singleModuleFixture.fastLinkJS + \
  singleModuleFixture.golemBuildComponent --component single --outPath "$out/mill.js" ||
  fail "Mill: golemBuildComponent failed on the single module fixture"
expect_full_link Mill "$out/mill.js" mill/out/singleModuleFixture/fullLinkJS.dest/main.js \
  mill/out/singleModuleFixture/fastLinkJS.dest/main.js

if [[ -f "$out/sbt.js" && -f "$out/mill.js" ]] && ! cmp -s "$out/sbt.js" "$out/mill.js"; then
  fail "sbt and Mill bundles of the same source differ: sbt $(describe "$out/sbt.js"), Mill $(describe "$out/mill.js")"
fi

echo "==> sbt: dynamic import fixture"
log=$(sbt_run "dynamicImportFixture/golemBuildComponent dynamic $out/sbt-dynamic.js" 2>&1)
status=$?
echo "$log"
expect_split_rejected sbt "$status" "$log" \
  test-build-plugins/dynamic-import/target/scala-3.3.7/dynamicimportfixture-opt "$out/sbt-dynamic.js"

echo "==> Mill: dynamic import fixture"
log=$(mill_run dynamicImportFixture.golemBuildComponent --component dynamic --outPath "$out/mill-dynamic.js" 2>&1)
status=$?
echo "$log"
expect_split_rejected Mill "$status" "$log" mill/out/dynamicImportFixture/fullLinkJS.dest "$out/mill-dynamic.js"

if ((${#failures[@]})); then
  echo "${#failures[@]} build plugin check(s) failed:" >&2
  printf '  - %s\n' "${failures[@]}" >&2
  exit 1
fi
echo "All build plugin checks passed."
