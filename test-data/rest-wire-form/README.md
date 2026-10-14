# REST wire form corpus

`v1.json` lists schema values in the REST wire form (`{"kind": …, "value": …}`), the encoding
that `ExternalSchemaValue` in `golem-common/src/schema/external.rs` reads and writes. Bridge
runtimes and SDK clients encode the same form independently, so each one checks itself against
this corpus instead of against another SDK.

- `roundtrip`: each `wire` value decodes, and encodes back to the same JSON value.
- `reject`: each `wire` value fails to decode.

The cases need no schema graph: the wire form names its kind on every node. Streams appear only
as a rejection of a stream without a session reference; a session's stream references are part of
the session protocol, not of this corpus.

Consumers:

- Rust (the host, and the source of truth): `shared_rest_wire_form_corpus_matches_the_host` in
  `golem-common/src/schema/external.rs`.
- Go: `TestSharedRestWireFormCorpus` in `sdks/go/core/schema/wirejson_test.go`.

When the host's wire form changes, change the corpus in the same commit, and the Rust test fails
until both agree.
