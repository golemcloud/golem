# MoonBit agent SDK test component

This application owns the MoonBit fixture used by the worker executor `agent_sdk_moonbit` tests.
Its `moon.work` resolves the SDK and generator from `../../sdks/moonbit`.

Build and copy the release artifact through the shared test-component build infrastructure with:

```shell
(cd .. && ./build-components.sh moonbit)
```

Run `moon fmt component`, `moon info`, and `moon check --target wasm` after changing MoonBit
source. Do not run an unscoped `moon fmt`: this workspace includes the in-repo SDKs. Always build
the component through the Golem CLI. The application must continue to produce
`../golem_it_agent_sdk_moonbit.wasm` and expose the `CustomDurability` agent.
