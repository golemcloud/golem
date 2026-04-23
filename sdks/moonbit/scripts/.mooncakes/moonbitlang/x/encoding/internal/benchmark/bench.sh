#!/usr/bin/env bash

set -e

build_moonbit() {
  cd encoding/internal/benchmark
  moon build --target all --release
  cd - > /dev/null
}
setup() {
  build_moonbit
  mkdir -p benchlog
}


setup

all_bench=$(find encoding/internal/benchmark/ -type d -name 'decoding*')
commit=$(git rev-parse --short HEAD)

echo -e "\n==== wasm ===="
cmds=()
for bench_path in $all_bench; do
  bench_name=$(basename $bench_path)
  seed=$(head -c 500 /dev/urandom | LC_ALL=C tr -dc 'a-zA-Z0-9~!@#$%^&*_-' | fold -w 32 | head -n 1)
  cmds+=("moonrun ./target/wasm/release/build/${bench_path}/${bench_name}.wasm '${seed}'")
  cmds+=("-n")
  cmds+=("wasm(v8) ${bench_name}")
done
hyperfine "${cmds[@]}" --export-json benchlog/wasm.${commit}.json

echo -e "\n==== wasm-gc ===="
cmds=()
for bench_path in $all_bench; do
  bench_name=$(basename $bench_path)
  seed=$(head -c 500 /dev/urandom | LC_ALL=C tr -dc 'a-zA-Z0-9~!@#$%^&*_-' | fold -w 32 | head -n 1)
  cmds+=("moonrun ./target/wasm-gc/release/build/${bench_path}/${bench_name}.wasm '${seed}'")
  cmds+=("-n")
  cmds+=("wasm-gc(v8) ${bench_name}")
done
hyperfine "${cmds[@]}" --export-json benchlog/wasm-gc.${commit}.json

echo -e "\n==== js ===="
cmds=()
for bench_path in $all_bench; do
  bench_name=$(basename $bench_path)
  seed=$(head -c 500 /dev/urandom | LC_ALL=C tr -dc 'a-zA-Z0-9~!@#$%^&*_-' | fold -w 32 | head -n 1)
  cmds+=("node ./target/js/release/build/${bench_path}/${bench_name}.js '${seed}'")
  cmds+=("-n")
  cmds+=("js(nodejs) ${bench_name}")
done
hyperfine "${cmds[@]}" --export-json benchlog/js.${commit}.json
