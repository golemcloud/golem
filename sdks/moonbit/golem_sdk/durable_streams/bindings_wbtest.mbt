///|
extern "wasm" fn set_host_mode(mode : Int) = "golem:test" "set-durable-stream-host-mode"

///|
test "generated async read append and clock imports preserve borrowed secret and u64 fields" {
  set_host_mode(1)
  defer set_host_mode(0)
  run_async(async fn() {
    let secret = @types.Secret::Secret(77)
    let read = @durable-streams.read_durable_stream_batch(
      {
        url: "https://example.test/stream",
        checkpoint: { offset: "now", cursor: None, },
        mode: JSON,
        transport: CATCH_UP,
        content_type: None,
        timeout_ms: 12345,
      },
      Some(secret),
    )
    let append = @durable-streams.append_durable_stream_batch(
      {
        url: "https://example.test/stream",
        content_type: "application/json",
        payload: Json(["18446744073709551615", "[1,2]"]),
        producer: { id: "producer", epoch: 4, sequence: 7, },
        close: true,
        timeout_ms: 12345,
      },
      Some(secret),
    )
    let expected : HostError = {
      kind: UNAVAILABLE,
      message: "https://example.test/stream",
      retry_after_ms: Some(987),
      producer_epoch: Some(9007199254740991),
      expected_sequence: Some(9007199254740990),
    }
    assert_eq(read, Err(expected))
    assert_eq(append, Err(expected))
    wait_ms(1)
    secret.drop()
  })
}

///|
test "append ABI preserves optional offset and acknowledges data and close without duplicate flag" {
  defer set_host_mode(0)
  for mode in [2, 3] {
    set_host_mode(mode)
    run_async(async fn() {
      let secret = @types.Secret::Secret(77)
      defer secret.drop()
      let writer : Writer[Int] = json_writer(
        "https://example.test/stream",
        producer_id="producer",
        epoch=9007199254740991,
        auth=secret,
        timeout_ms=12345,
      )
      let expected_offset = if mode == 2 {
        None
      } else {
        Some("https://example.test/stream")
      }
      let appended = writer.append([19, 2])
      assert_eq(appended.next_offset, expected_offset)
      assert_eq(appended.epoch, 9007199254740991)
      assert_eq(appended.sequence, 0)
      assert_false(appended.closed)
      assert_false(writer.has_pending())
      assert_eq(writer.producer.sequence, 1)
      let closed = writer.close()
      assert_eq(closed.next_offset, expected_offset)
      assert_eq(closed.epoch, 9007199254740991)
      assert_eq(closed.sequence, 1)
      assert_true(closed.closed)
      assert_false(writer.has_pending())
      assert_true(writer.closed)
      assert_eq(writer.producer.sequence, 2)
    })
  }
}
