#!/usr/bin/env bash

set -e

seed() {
  head -c 500 /dev/urandom | LC_ALL=C tr -dc 'a-zA-Z0-9~!@#$%^&*_-' | fold -w 32 | head -n 1
}
hard_reset() {
  cd $1
  git reset --hard $2
  # git cherry-pick af684bdbe64a9ec816e2a841adcf86775770a23a
  cd - > /dev/null
}
build_moonbit() {
  echo "building ${1}/encoding/internal/benchmark"
  moon build --target all --release -C "${1}/encoding/internal/benchmark"
}

COMMIT_COMPARE=$1
COMMIT_CURRENT=$(git rev-parse --short HEAD)

if [ -z "$COMMIT_COMPARE" ]; then
  echo "Error: No commit to compare provided." >&2
  exit 1
fi

echo "comparing with $COMMIT_COMPARE"

d_compare=$(mktemp -d /tmp/x-bench-${COMMIT_COMPARE}-XXXXX)
git clone . $d_compare
hard_reset $d_compare $COMMIT_COMPARE

build_moonbit $d_compare
build_moonbit .
mkdir -p benchlog

all_bench=$(find encoding/internal/benchmark/ -type d -name 'decoding*')

for bench_path in $all_bench; do
  bench_name=$(basename $bench_path)

  seed=$(seed)
  hyperfine \
    "moonrun ./target/wasm/release/build/${bench_path}/${bench_name}.wasm '${seed}'" \
    -n "wasm ${COMMIT_CURRENT} (HEAD) ${bench_name}" \
    "moonrun ${d_compare}/target/wasm/release/build/${bench_path}/${bench_name}.wasm '${seed}'" \
    -n "wasm ${COMMIT_COMPARE} ${bench_name}" \
    --warmup 5 \
    --export-markdown benchlog/wasm.${bench_name}.${COMMIT_CURRENT}.${COMMIT_COMPARE}.md

  seed=$(seed)
  hyperfine \
    "moonrun ./target/wasm-gc/release/build/${bench_path}/${bench_name}.wasm '${seed}'" \
    -n "wasm-gc ${COMMIT_CURRENT} (HEAD) ${bench_name}" \
    "moonrun ${d_compare}/target/wasm-gc/release/build/${bench_path}/${bench_name}.wasm '${seed}'" \
    -n "wasm-gc ${COMMIT_COMPARE} ${bench_name}" \
    --warmup 5 \
    --export-markdown benchlog/wasm-gc.${bench_name}.${COMMIT_CURRENT}.${COMMIT_COMPARE}.md

  seed=$(seed)
  hyperfine \
    "node ./target/js/release/build/${bench_path}/${bench_name}.js '${seed}'" \
    -n "js ${COMMIT_CURRENT} (HEAD) ${bench_name}" \
    "node ${d_compare}/target/js/release/build/${bench_path}/${bench_name}.js '${seed}'" \
    -n "js ${COMMIT_COMPARE} ${bench_name}" \
    --warmup 5 \
    --export-markdown benchlog/js.${bench_name}.${COMMIT_CURRENT}.${COMMIT_COMPARE}.md
done

rm $d_compare -rf
(for file in benchlog/wasm.decoding*.md; do cat "$file"; echo; done) > "benchlog/wasm.${COMMIT_CURRENT}.${COMMIT_COMPARE}.md"
(for file in benchlog/wasm-gc.decoding*.md; do cat "$file"; echo; done) > "benchlog/wasm-gc.${COMMIT_CURRENT}.${COMMIT_COMPARE}.md"
(for file in benchlog/js.decoding*.md; do cat "$file"; echo; done) > "benchlog/js.${COMMIT_CURRENT}.${COMMIT_COMPARE}.md"
