#### Build and Test

```sh
npx pnpm clean # if needed
npx pnpm install
npx pnpm run format
npx pnpm run format:check
npx pnpm run lint
npx pnpm run build
npx pnpm run test # or per package: Ex: cd packages/golem-ts-sdk && pnpm run test
```

`pnpm run test` within packages will run tests without forgetting console logs, and will be more faster.

## Self metadata and restrictive permissions

`getSelfMetadata()` requires view permission on the running agent:
`agent(?agent) @ ?agent : view : *`. The effective permission card must admit that
operation and the `initialCard` upper bound must permit it. Permission denials name
`get-self-metadata`, explain the required permission, and retain the original host
error in `Error.cause`. Backend errors also identify the operation and backend cause.
These errors remain readable when atomic or Saga wrappers convert them to trap text.

Pure initialization does not implicitly read self metadata. Keep metadata calls out
of an untrusted evaluator's initialization and handler rather than adding self-view
as a bootstrap grant. Declare no tools or secrets and admit only the host operations
the evaluator actually needs within its upper bound.

Normal agent initialization, `getRawSelfAgentId()`, and snapshot loading read
`GOLEM_AGENT_ID` through the WASI environment API, not through self metadata. They
require environment-read permission for that variable; self-view does not provide it.

Self metadata includes stored environment and configuration fields without the
per-key permission filtering used by WASI environment/configuration reads. A denied
or empty WASI read does not prove those fields are inaccessible through metadata.
Self-view is not an identity-only permission; keep it denied for untrusted code that
must not read those fields. Do not log full metadata when diagnosing a denial.
