# GOL-40 conformance artifacts

This directory contains language-neutral contracts shared by GOL-40 tests. These fixtures are
test data, not production compatibility promises.

- `rich-tool-conformance-v1.json` is the language-neutral native-tool contract used by the SDK,
  canonical-metadata, generated-client, middleware-transparency, and native-source chunks.
- `mcp-projection-v1.json` is the deliberately lossy MCP projection contract. It defines native
  export and MCP import separately; neither may be compared with the native contract as though the
  representations were lossless.

## Consumption rules

Consumers must compare each independently emitted metadata record with
`rich-tool-conformance-v1.json`; one SDK's output must not become another SDK's expected value.
Canonicalization is limited to `comparison.knownSemanticallyUnorderedCollections`. Any additional
normalization requires a demonstrated representation difference and an update to this fixture.

MCP tests consume `mcp-projection-v1.json` instead. They must assert the projected contract and the
documented losses, not native/MCP equality.
