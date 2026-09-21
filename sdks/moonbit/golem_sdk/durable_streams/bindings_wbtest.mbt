///|
extern "wasm" fn set_host_mode(mode : Int) = "golem:test" "set-durable-stream-host-mode"

///|
#owned(value)
extern "wasm" fn string_pointer(value : String) -> Int =
  #|(func (param i32) (result i32) local.get 0)

///|
#owned(value)
extern "wasm" fn bytes_pointer(value : Bytes) -> Int =
  #|(func (param i32) (result i32) local.get 0)

///|
extern "wasm" fn reply_string(pointer : Int, length : Int) = "golem:test" "durable-stream-reply-string"

///|
extern "wasm" fn resource_stat(handle : Int, field : Int) -> Int = "golem:test" "durable-stream-resource-stat"

///|
fn reply(value : String) -> Unit {
  reply_string(string_pointer(value), value.length())
}

///|
test "native reads reuse descriptor and lower independent checkpoint tokens after draining" {
  set_host_mode(4)
  defer set_host_mode(0)
  run_async(async fn() {
    let count = resource_stat(0, 0)
    let secret = @types.Secret::Secret(77)
    let reader : Reader[Int] = json_reader(
      "https://example.test/stream",
      offset="now",
      transport=SSE,
      auth=secret,
      timeout_ms=12345,
    )
    secret.drop()
    let @durable-streams.DurableStreamReader(handle) = reader.resource.unwrap()
    for payload in [b"[13,4]", b"[9]"] {
      reply_string(bytes_pointer(payload), payload.length())
      reply("application/json")
      reply("opaque:next")
      reply("independent:cursor")
    }
    assert_eq(reader.read(), Some(13))
    assert_eq(reader.checkpoint().offset, "now")
    assert_eq(resource_stat(handle, 2), 1)
    assert_eq(reader.read(), Some(4))
    assert_eq(resource_stat(handle, 2), 1)
    assert_eq(reader.checkpoint().offset, "opaque:next")
    assert_eq(reader.checkpoint().cursor, Some("independent:cursor"))
    assert_eq(reader.read(), Some(9))
    assert_eq(reader.read(), None)
    assert_eq(resource_stat(handle, 2), 2)
    assert_eq(resource_stat(handle, 3), 1)
    assert_eq(resource_stat(0, 0), count + 1)
  })
}

///|
test "resource constructors capture borrowed secret once and explicit retries reuse handles" {
  set_host_mode(1)
  defer set_host_mode(0)
  run_async(async fn() {
    let readers_before = resource_stat(0, 0)
    let writers_before = resource_stat(0, 1)
    let secret = @types.Secret::Secret(77)
    let reader : Reader[Int] = json_reader(
      "https://example.test/stream",
      offset="now",
      auth=secret,
      timeout_ms=12345,
      max_retries=0,
    )
    let writer : Writer[String] = json_writer_with_codec(
      "https://example.test/stream",
      value => value,
      producer_id="producer",
      epoch=9007199254740991,
      auth=secret,
      timeout_ms=12345,
      max_retries=0,
    )
    secret.drop()
    let @durable-streams.DurableStreamReader(read_handle) = reader.resource.unwrap()
    let @durable-streams.DurableStreamWriter(write_handle) = writer.resource.unwrap()
    assert_eq(resource_stat(0, 0), readers_before + 1)
    assert_eq(resource_stat(0, 1), writers_before + 1)
    assert_eq(resource_stat(read_handle, 2), 0)
    assert_eq(resource_stat(write_handle, 2), 0)
    let expected : HostError = {
      kind: UNAVAILABLE,
      message: "outage",
      retry_after_ms: Some(987),
      producer_epoch: Some(9007199254740991),
      expected_sequence: Some(9007199254740990),
    }
    for _ in 0..<2 {
      reply("outage")
      try reader.read() catch {
        DurableStreamError::Remote(error) => assert_eq(error, expected)
        error => raise error
      } noraise {
        _ => fail("outage became EOF")
      }
    }
    reply("outage")
    try writer.append(["18446744073709551615", "[1,2]"]) catch {
      DurableStreamError::Remote(error) => assert_eq(error, expected)
      error => raise error
    } noraise {
      _ => fail("uncertain append acknowledged")
    }
    let pending = writer.pending
    assert_true(writer.has_pending())
    set_host_mode(2)
    let receipt = writer.retry_pending()
    assert_eq(receipt.next_offset, None)
    assert_eq(receipt.epoch, 9007199254740991)
    assert_eq(receipt.sequence, pending.unwrap().sequence)
    assert_false(writer.has_pending())
    assert_eq(resource_stat(read_handle, 2), 2)
    assert_eq(resource_stat(write_handle, 2), 2)
    assert_eq(resource_stat(0, 0), readers_before + 1)
    assert_eq(resource_stat(0, 1), writers_before + 1)
    reader.close()
    reader.close()
    writer.drop()
    writer.drop()
    assert_eq(resource_stat(read_handle, 3), 1)
    assert_eq(resource_stat(write_handle, 3), 1)
    set_host_mode(1)
    wait_ms(1)
  })
}

///|
test "append resource ABI preserves optional offsets and drops after close-only acknowledgement" {
  defer set_host_mode(0)
  for mode in [2, 3] {
    set_host_mode(mode)
    run_async(async fn() {
      let secret = @types.Secret::Secret(77)
      let writer : Writer[Int] = json_writer(
        "https://example.test/stream",
        producer_id="producer",
        epoch=9007199254740991,
        auth=secret,
        timeout_ms=12345,
      )
      secret.drop()
      let @durable-streams.DurableStreamWriter(handle) = writer.resource.unwrap()
      let expected_offset = if mode == 2 { None } else { Some("opaque:next") }
      if mode == 3 {
        reply("opaque:next")
      }
      let appended = writer.append([19, 2])
      assert_eq(appended.next_offset, expected_offset)
      assert_eq(appended.epoch, 9007199254740991)
      assert_eq(appended.sequence, 0)
      assert_false(appended.closed)
      assert_eq(resource_stat(handle, 3), 0)
      if mode == 3 {
        reply("opaque:next")
      }
      let closed = writer.close()
      assert_eq(closed.next_offset, expected_offset)
      assert_eq(closed.epoch, 9007199254740991)
      assert_eq(closed.sequence, 1)
      assert_true(closed.closed)
      assert_false(writer.has_pending())
      assert_true(writer.closed)
      assert_eq(writer.sequence, 2)
      assert_eq(resource_stat(handle, 2), 2)
      assert_eq(resource_stat(handle, 3), 1)
      writer.drop()
    })
  }
}

///|
test "distinct reader handles release on unstarted native stream drop and final payload" {
  run_async(async fn() {
    let first : Reader[Int] = json_reader("https://example.test/first")
    let second : Reader[Int] = json_reader("https://example.test/second")
    let @durable-streams.DurableStreamReader(handle1) = first.resource.unwrap()
    let @durable-streams.DurableStreamReader(handle2) = second.resource.unwrap()
    assert_true(handle1 != handle2)
    first.into_stream().drop()
    assert_eq(resource_stat(handle1, 3), 1)
    second.install(batch(b"[13,4]", "end", closed=true))
    let stream = second.into_stream()
    assert_eq(stream.read(), Some(13))
    assert_eq(stream.read(), Some(4))
    assert_eq(stream.read(), None)
    assert_eq(resource_stat(handle2, 3), 1)
  })
}
