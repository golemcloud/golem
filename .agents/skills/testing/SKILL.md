---
name: testing
description: "Running and debugging tests in the Golem workspace. Use when writing tests, running specific tests, filtering tests, debugging test failures, or understanding test infrastructure."
---

# Testing in Golem

Tests use [test-r](https://test-r.vigoo.dev). Each test file **must** import `test_r::test` or the tests will silently not run:

```rust
use test_r::test;

#[test]
fn my_test() {
    // ...
}
```

## Choosing the Right Test Command

**Do not run `cargo make test`** — it runs all tests and takes a very long time.

| Change Type | Test Command |
|-------------|--------------|
| Core logic, utilities | `cargo make unit-tests` |
| Worker executor functionality | `cargo make worker-executor-tests` |
| Service integration | `cargo make integration-tests` |
| CLI changes | `cargo make cli-tests` |

**Whenever tests are modified, always run the affected tests to verify they still pass before considering the task complete.**

For running specific tests during development:
```shell
cargo test -p <crate> -- <test_name> --report-time
```

## Test Filtering Rules (test-r)

This project uses `test-r` which supports **multiple filter arguments after `--`**. Filters are OR-matched (a test runs if it matches any filter). Each filter is a **substring match**, not a regex.

```shell
# Run a single specific test:
cargo test -p <crate> -- <test_name> --report-time

# Run multiple specific tests (filters go AFTER --, not before):
cargo test -p <crate> -- test_name_1 test_name_2 test_name_3 --report-time

# WRONG - multiple filters before -- causes "unexpected argument" error:
# cargo test -p <crate> test1 test2 -- --report-time

# WRONG - regex patterns don't work (filters are substring matches, not regex):
# cargo test -p <crate> -- "test_a|test_b" --report-time
# cargo test -p <crate> -- "test_.*pattern" --report-time
```

**Note:** `--list` in test-r ignores filters and always lists all tests. Do not use `--list` to verify that filters are working. Instead, do a real run and check the `filtered out` count in the result line.

## Debugging Test Failures

Use `--nocapture` when debugging tests:
```shell
cargo test -p <crate> -- <test> --nocapture
```

**Always save test output to a file** when running worker executor tests, integration tests, or CLI tests. These tests are slow and produce potentially thousands of lines of logs. Never pipe output directly to `grep`, `head`, `tail`, etc. — if you need to examine different parts of the output, you would have to re-run the entire slow test. Instead:
```shell
cargo test -p <crate> -- <test> --nocapture > tmp/test_output.txt 2>&1
# Then search/inspect the saved file as needed
grep -n "pattern" tmp/test_output.txt
```

**Handling hanging tests:** Load the `debugging-hanging-tests` skill for a step-by-step workflow.

## Test Components

Worker executor tests and integration tests use pre-compiled WASM files from the `test-components/` directory. These are checked into the repository and **rebuilding them is not automated**. Do not attempt to rebuild test components — use the existing compiled WASM files, EXCEPT if the test component itself has an AGENTS.md file with instructions of how to do so.

Load the `modifying-test-components` skill when rebuilding is needed.

## Timeouts

Add a `#[timeout]` attribute for tests that should fail rather than hang:

```rust
use test_r::test;
use test_r::timeout;

#[test]
#[timeout("30s")]
async fn my_test() {
    // ...
}
```

Choose a timeout generous enough for normal execution but short enough to fail quickly when hung (30s–60s for most tests, up to 120s for complex integration tests).
