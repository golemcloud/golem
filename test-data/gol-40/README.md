# GOL-40 conformance artifacts

This directory is the shared source of truth for the remaining GOL-40 acceptance work.
It contains data, not a production compatibility promise.

- `acceptance-ledger-v1.json` maps every named test-plan row to its current state. Existing
  coverage is credited only when an exact test and an independent oracle are named.
- `rich-tool-conformance-v1.json` is the language-neutral native-tool contract used by the SDK,
  canonical-metadata, generated-client, middleware-transparency, and native-source chunks.
- `mcp-projection-v1.json` is the deliberately lossy MCP projection contract. It defines native
  export and MCP import separately; neither may be compared with the native contract as though the
  representations were lossless.

## Ledger statuses

- `covered`: the complete row already has an equally strong test and independent oracle.
- `new assertion`: an existing test boundary can cover the row, but assertions or cases are missing.
- `new integration boundary`: unit or adjacent coverage exists, but the required deployed,
  cross-language, cross-service, or composed boundary does not.
- `requires implementation`: production behavior or a product/persistence decision is missing.

The status is for the complete row, not its strongest existing subcase. Consequently a row remains
`new assertion` or `new integration boundary` when the ledger names substantial baseline coverage.

## Consumption rules

Consumers must compare each independently emitted metadata record with
`rich-tool-conformance-v1.json`; one SDK's output must not become another SDK's expected value.
Canonicalization is limited to `comparison.knownSemanticallyUnorderedCollections`. Any additional
normalization requires a demonstrated representation difference and an update to this fixture.

MCP tests consume `mcp-projection-v1.json` instead. They must assert the projected contract and the
documented losses, not native/MCP equality.
