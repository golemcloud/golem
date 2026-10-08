#!/bin/sh
# Runs Brush's compatibility suite (about 2,600 YAML cases compared with a real bash) against the
# bash tool's shell under Wasmtime.
#
#   run.sh BRUSH_CHECKOUT COMPAT_WASM [ORACLE_BASH] [-- harness arguments...]
#
# BRUSH_CHECKOUT is the Brush fork at the rev pinned in ../../Cargo.toml; COMPAT_WASM is the
# `compat` example built for wasm32-wasip2. The harness clears the environment, so the launcher is
# written with absolute paths.
set -eu
brush=$(cd "$1" && pwd); wasm=$(cd "$(dirname "$2")" && pwd)/$(basename "$2"); shift 2
bash_path=$(command -v bash)
if [ $# -gt 0 ] && [ "$1" != "--" ]; then bash_path=$1; shift; fi
[ $# -gt 0 ] && [ "$1" = "--" ] && shift

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
wasmtime=$(command -v wasmtime)
mktemp_bin=$(command -v mktemp)
# Compile once: faster than compiling per case, and nothing is cached under the case's HOME.
"$wasmtime" compile -W component-model-async=y -o "$work/compat.cwasm" "$wasm"
cat >"$work/launcher" <<LAUNCHER
#!/bin/sh
# The compat example as a native shell binary: real exit status, a private /tmp, the caller's cwd,
# and a root directory of its own, as an agent always has one (the shell keeps scratch files there).
wasm=\$1; shift
state=\$($mktemp_bin -d) || exit 125
mkdir "\$state/tmp" "\$state/root"
"$wasmtime" run --allow-precompiled -Ccache=n -Sp3 -Shttp -Wcomponent-model-async=y -Sinherit-env \\
  --dir "\$state/root::/" --dir "\$PWD::\$PWD" --dir "\$state/tmp::/tmp" --dir "\$state::/.compat" \\
  "\$wasm" --status-file /.compat/status --cwd "\$PWD" "\$@"
code=\$(cat "\$state/status" 2>/dev/null || echo 1)
rm -rf "\$state"
exit "\$code"
LAUNCHER
chmod +x "$work/launcher"

cd "$brush"
${CARGO:-cargo +stable} build -q -p brush-shell --test brush-compat-tests 2>/dev/null || true
harness=$(${CARGO:-cargo +stable} test -p brush-shell --test brush-compat-tests --no-run --message-format=json 2>/dev/null \
  | sed -n 's/.*"executable":"\([^"]*brush_compat_tests[^"]*\)".*/\1/p' | tail -n 1)
"$harness" --bash-path "$bash_path" --brush-path "$work/compat.cwasm" --brush-launcher "$work/launcher" \
  --brush-platform-tags "wasi wasm golem" --test-cases-path "$brush/brush-shell/tests/cases/compat" "$@"
