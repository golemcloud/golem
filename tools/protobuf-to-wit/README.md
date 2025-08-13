# protobuf-to-wit (gRPC → WIT)

A small generator library that converts Protobuf (proto3) service/message definitions into WIT (WebAssembly Interface Types).

Scope (bounty-aligned)
- messages → records
- oneof → variant
- service → interface (RPCs map to `func(request) -> result<response, error>`)
- Deterministic kebab-case naming; reserved WIT words prefixed

Usage
- Library:
  - `protobuf_to_wit::convert_protobuf_to_wit(proto_src: &str) -> WitOutput`
- Tests:
  - `cargo test -p protobuf-to-wit`

Notes
- Error variant is placeholder for now; refined mapping can be introduced later per platform error model.
- Input: single `.proto` source string. Descriptor-set-based multi-file resolution can be added in a follow-up. 