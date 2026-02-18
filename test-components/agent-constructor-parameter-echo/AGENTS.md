# agent-constructor-parameter-echo Test Component

This is a Golem Application test component used by worker executor tests.

## Prerequisites

- Node.js and npm
- The `golem` CLI built from the repo (at `target/debug/golem`)
- If the TS SDK source was modified, rebuild the SDK **including the agent template wasm** first: run `npx pnpm run build && npx pnpm run build-agent-template` in `sdks/ts/`

## Building

From within the `test-components/agent-constructor-parameter-echo/` directory, build:

```shell
../../target/debug/golem build --force-build
```

Use the locally built golem binary from the repo, not an installed version, to ensure version compatibility.

Then copy the output to the test-components directory:

```shell
cp golem-temp/agents/golem_it_constructor_parameter_echo.wasm ../
```

The compiled WASM file `test-components/golem_it_constructor_parameter_echo.wasm` is checked into the repository.
