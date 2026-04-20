# Moonbit/Core Encoding Benchmarking

## Pre-requisites
- [hyperfine](https://github.com/sharkdp/hyperfine)
- NodeJS

## Run

Benchmark the current state of the code:
```sh
./encoding/internal/benchmark/bench.sh
```

Benchmarks that compare code from different commits, example:
```sh
./encoding/internal/benchmark/bench_diff.sh HEAD~1
```
