// Phase 3f tests for the schema-carrier quota-token impls.
//
// The runtime `QuotaToken` methods (`reserve`/`split`/`merge`/`to_record`/
// `from_record`/`drop`/`commit`) call WASM host imports and therefore cannot
// run in a unit test — referencing them links the `golem:quota/types` host
// import, which the test runtime cannot instantiate. (`to_value` calls
// `to_record` and `from_value` calls `from_record`, so neither value-side path
// is host-free either.) These tests cover the type-side capability schema
// (`type_id` + `register_in`, both pure) and its WIT round-trip.

///|
test "QuotaToken schema is a single capability node with no defs" {
  let graph = @schema.into_schema_graph(
    (@schema.type_tag() : @schema.TypeTag[QuotaToken]),
  )
  inspect(graph.defs.length(), content="0")
  inspect(
    graph.root.body,
    content=(
      #|QuotaToken({resource_name: None})
    ),
  )
}

///|
test "QuotaToken schema survives the WIT round-trip" {
  let graph = @schema.into_schema_graph(
    (@schema.type_tag() : @schema.TypeTag[QuotaToken]),
  )
  let wit = @model.schema_graph_to_wit(graph)
  let back = @model.schema_graph_from_wit(wit)
  assert_eq(back, graph)
}

///|
/// The value-side `to_value`/`from_value` impls cannot be exercised host-free
/// (they call `to_record`/`from_record`), but the pure record↔payload mapping
/// they delegate to can. These cover the signedness bridge and the
/// non-negative-timestamp guard without any host import.
test "record -> payload -> record round-trips for a valid timestamp" {
  let record : @raw.QuotaTokenRecord = {
    environment_id: @host.EnvironmentId::{ uuid: { high_bits: 7, low_bits: 9 } },
    resource_name: "llm-tokens",
    expected_use: 1000,
    last_credit: -42,
    last_credit_at: @wallClock.Datetime::{
      seconds: 1_700_000_000,
      nanoseconds: 500,
    },
  }
  let payload = quota_token_payload_from_record(record)
  inspect(payload.last_credit_at.seconds, content="1700000000")
  let back = quota_token_record_from_payload(payload)
  assert_eq(back, record)
}

///|
test "payload -> record rejects a negative timestamp" {
  let payload : @types.QuotaTokenValuePayload = {
    environment_id: { uuid: { high_bits: 0, low_bits: 0 } },
    resource_name: "r",
    expected_use: 0,
    last_credit: 0,
    last_credit_at: { seconds: -1, nanoseconds: 0 },
  }
  let r : Result[@raw.QuotaTokenRecord, @schema.FromSchemaError] = try? quota_token_record_from_payload(
    payload,
  )
  guard r is Err(Custom(msg)) else { fail("expected Custom error") }
  inspect(msg, content="quota token timestamp must be non-negative")
}
