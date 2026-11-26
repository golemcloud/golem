# Integration tests

- Use `golem-test-framework::dsl::TestDsl` as the main test interface. Extend the DSL for repeated deployment or invocation patterns; each new DSL helper must serve at least two tests. Process-backed dependencies belong to `golem-test-framework`.
- Write tests as setup, action, then checks. Split independent scenarios into separate tests. Give phases that must share state across a real service restart their own named functions.
- Return fixtures as structs with named fields, not positional tuples. Keep service and server handles with the fixture so their lifetime is visible.
- Build schema types and values with `IntoSchema`-derived structs, `data_value!`, or `IntoTypedSchemaValue`, rather than hand-built schema/value ADTs.
- Group checks by the guarantee they prove, using the testing-plan wording when available. Explain non-obvious assertions with messages. Match typed error variants or error codes, not substrings of formatted errors.
- Put imports at module scope and use short imported paths. Name timeouts and oplog positions instead of repeating literals.
- Use shared helpers for oplog-shape checks: one `Finished` per key; each `Start` in settled history has a terminal except at an explicit crash or cancellation window; no positional `Start`/`End` after `Settled`, or after the invocation terminal for invocations without `Settled`. Preserve the retained-call, deferred-destructor and safe-park exceptions described in `golem-worker-executor-tests/tests/AGENTS.md`.
- A readability refactor must preserve every scenario, crash window, effect counter and assertion. Run the same affected tests before and after it. Test execution mechanics are in the `testing` skill.
