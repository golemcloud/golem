///|
fn run_agent_stream_async(body : async () -> Unit) -> Unit raise {
  let mut callback = @async-core.with_waitableset(async fn() {
    body()
    @async-core.task_returned()
  })
  let mut steps = 0
  while callback != 0 {
    steps = steps + 1
    if steps > 1000 {
      fail("agent stream async test did not complete")
    }
    if (callback & 0xf) != 1 {
      fail("agent stream async test unexpectedly waited on a host event")
    }
    callback = @async-core.cb(0, 0, 0)
  }
}

///|
extern "wasm" fn reset_resource_drop_counts() = "golem:test" "reset-resource-drop-counts"

///|
extern "wasm" fn resource_drop_count(kind : Int) -> Int = "golem:test" "resource-drop-count"

///|
extern "wasm" fn set_schema_value_stream_host_mode(mode : Int) = "golem:test" "set-schema-value-stream-host-mode"

///|
fn fake_agent_stream(id : Int) -> AgentStream[Int] {
  {
    handle: @model.GuestSchemaValueStreamHandle::from_wrapped(
      @types.SchemaValueStream::SchemaValueStream(id),
    ),
    decode: from_value_as,
  }
}

///|
priv enum EncodeProbe {
  EncodeCapability(AgentStream[Int])
  EncodeInvalid
}

///|
impl FromSchema for EncodeProbe with fn from_value(value) {
  EncodeCapability(from_value_as(value))
}

///|
impl IntoSchema for EncodeProbe with fn type_id() {
  "test.EncodeProbe"
}

///|
impl IntoSchema for EncodeProbe with fn register_in(_builder) {
  @model.schema_type(@model.SchemaTypeBody::S8(None))
}

///|
impl IntoSchema for EncodeProbe with fn to_value(self) {
  match self {
    EncodeCapability(stream) => to_value_as(stream)
    EncodeInvalid => @model.SchemaValue::S8(999)
  }
}

///|
test "typed producer closes normally and preserves order" {
  run_agent_stream_async(async fn() {
    let stream : AgentStream[Int] = AgentStream::produce(async fn(writer) {
      assert_true(writer.write_one(1) is Accepted)
      assert_true(writer.write_all([2, 3]) is Accepted)
    })
    assert_eq(stream.read(), Some(1))
    assert_eq(stream.read(), Some(2))
    assert_eq(stream.read(), Some(3))
    assert_true(stream.read() is None)
  })
}

///|
test "explicit close is idempotent and produces clean EOF" {
  run_agent_stream_async(async fn() {
    let (writer, stream) : (AgentStreamWriter[Int], AgentStream[Int]) = AgentStream::new()
    writer.close()
    writer.close()
    assert_true(stream.read() is None)
  })
}

///|
test "accepted write waits for reader demand" {
  run_agent_stream_async(async fn() {
    let (writer, stream) : (AgentStreamWriter[Int], AgentStream[Int]) = AgentStream::new()
    let (started, started_promise) = @async-core.Future::new()
    let completed = [false]
    @async-core.with_task_group(async fn(group) {
      let write = group.spawn(async fn() {
        ignore(started_promise.complete(()))
        let outcome = writer.write_one(9)
        completed[0] = true
        outcome
      })
      started.get()
      assert_false(completed[0])
      assert_eq(stream.read(), Some(9))
      assert_true(write.wait() is Accepted)
    })
    writer.close()
    assert_true(stream.read() is None)
  })
}

///|
test "partial write_all owns and cleans every nested stream" {
  reset_resource_drop_counts()
  run_agent_stream_async(async fn() {
    let values = Array::makei(70, fake_agent_stream)
    let (writer, stream) : (
      AgentStreamWriter[AgentStream[Int]],
      AgentStream[AgentStream[Int]],
    ) = AgentStream::new()
    @async-core.with_task_group(async fn(group) {
      let write = group.spawn(async fn() { writer.write_all(values) })
      match stream.read() {
        Some(first) => first.drop()
        None => fail("partial write produced no first item")
      }
      stream.drop()
      assert_true(write.wait() is PeerDropped)
    })
  })
  assert_eq(resource_drop_count(3), 70)
}

///|
test "cancelled writes propagate cancellation and clean pending items" {
  reset_resource_drop_counts()
  run_agent_stream_async(async fn() {
    let value = fake_agent_stream(100)
    let (writer, stream) : (
      AgentStreamWriter[AgentStream[Int]],
      AgentStream[AgentStream[Int]],
    ) = AgentStream::new()
    let (started, started_promise) = @async-core.Future::new()
    @async-core.with_task_group(async fn(group) {
      let write = group.spawn(async fn() {
        ignore(started_promise.complete(()))
        writer.write_one(value)
      })
      started.get()
      write.cancel()
      try write.wait() catch {
        @async-core.Cancelled::Cancelled => ()
        _ => fail("writer cancellation changed error category")
      } noraise {
        _ => fail("cancelled writer unexpectedly completed")
      }
    })
    stream.drop()
  })
  assert_eq(resource_drop_count(3), 1)
}

///|
test "encoding failure cleans prior and unattempted capability items" {
  reset_resource_drop_counts()
  run_agent_stream_async(async fn() {
    let (writer, stream) : (
      AgentStreamWriter[EncodeProbe],
      AgentStream[EncodeProbe],
    ) = AgentStream::new()
    try
      writer.write_all([
        EncodeCapability(fake_agent_stream(110)),
        EncodeInvalid,
        EncodeCapability(fake_agent_stream(111)),
      ])
    catch {
      @model.SchemaModelError::EncodeError(_) => ()
      error => fail("unexpected encoding failure: \{Repr(error)}")
    } noraise {
      _ => fail("invalid item unexpectedly encoded")
    }
    writer.close()
    stream.drop()
  })
  assert_eq(resource_drop_count(3), 2)
}

///|
test "cancellation while wrapping a nested stream stays canonical" {
  reset_resource_drop_counts()
  let cleanup_runs = [0]
  let producer_started = [false]
  run_agent_stream_async(async fn() {
    set_schema_value_stream_host_mode(3)
    defer set_schema_value_stream_host_mode(0)
    let captured = fake_agent_stream(120)
    let nested : AgentStream[Int] = AgentStream::produce(
      async fn(writer) {
        producer_started[0] = true
        captured.drop()
        writer.close()
      },
      on_unstarted_drop=fn() {
        cleanup_runs[0] += 1
        captured.drop()
      },
    )
    let (writer, stream) : (
      AgentStreamWriter[AgentStream[Int]],
      AgentStream[AgentStream[Int]],
    ) = AgentStream::new()
    let (started, started_promise) = @async-core.Future::new()
    @async-core.with_task_group(async fn(group) {
      let write = group.spawn(async fn() {
        ignore(started_promise.complete(()))
        writer.write_one(nested)
      })
      started.get()
      write.cancel()
      try write.wait() catch {
        @async-core.Cancelled::Cancelled => ()
        error =>
          fail("nested wrap cancellation changed category: \{Repr(error)}")
      } noraise {
        _ => fail("cancelled nested wrap unexpectedly completed")
      }
    })
    writer.close()
    stream.drop()
  })
  assert_false(producer_started[0])
  assert_eq(cleanup_runs[0], 1)
  assert_eq(resource_drop_count(3), 1)
}

///|
test "unstarted producer cleanup runs exactly once" {
  reset_resource_drop_counts()
  let producer_started = [false]
  let cleanup_runs = [0]
  let captured = fake_agent_stream(130)
  let stream : AgentStream[Int] = AgentStream::produce(
    async fn(writer) {
      producer_started[0] = true
      captured.drop()
      writer.close()
    },
    on_unstarted_drop=fn() {
      cleanup_runs[0] += 1
      captured.drop()
    },
  )
  stream.drop()
  stream.drop()
  assert_false(producer_started[0])
  assert_eq(cleanup_runs[0], 1)
  assert_eq(resource_drop_count(3), 1)
}

///|
test "starting a producer clears its unstarted cleanup" {
  reset_resource_drop_counts()
  let producer_started = [false]
  let cleanup_runs = [0]
  run_agent_stream_async(async fn() {
    let captured = fake_agent_stream(131)
    let stream : AgentStream[Int] = AgentStream::produce(
      async fn(writer) {
        producer_started[0] = true
        captured.drop()
        writer.close()
      },
      on_unstarted_drop=fn() {
        cleanup_runs[0] += 1
        captured.drop()
      },
    )
    assert_true(stream.read() is None)
    stream.drop()
  })
  assert_true(producer_started[0])
  assert_eq(cleanup_runs[0], 0)
  assert_eq(resource_drop_count(3), 1)
}

///|
test "cancelled reads propagate canonical cancellation" {
  run_agent_stream_async(async fn() {
    let (writer, stream) : (AgentStreamWriter[Int], AgentStream[Int]) = AgentStream::new()
    let (started, started_promise) = @async-core.Future::new()
    @async-core.with_task_group(async fn(group) {
      let read = group.spawn(async fn() {
        ignore(started_promise.complete(()))
        stream.read()
      })
      started.get()
      read.cancel()
      try read.wait() catch {
        @async-core.Cancelled::Cancelled => ()
        _ => fail("reader cancellation changed error category")
      } noraise {
        _ => fail("cancelled reader unexpectedly completed")
      }
    })
    writer.close()
    stream.drop()
  })
}

///|
test "cancellation while unwrapping a stream stays canonical and drops it" {
  reset_resource_drop_counts()
  run_agent_stream_async(async fn() {
    set_schema_value_stream_host_mode(4)
    defer set_schema_value_stream_host_mode(0)
    let stream = fake_agent_stream(190)
    let (started, started_promise) = @async-core.Future::new()
    @async-core.with_task_group(async fn(group) {
      let read = group.spawn(async fn() {
        ignore(started_promise.complete(()))
        stream.read()
      })
      started.get()
      read.cancel()
      try read.wait() catch {
        @async-core.Cancelled::Cancelled => ()
        error => fail("stream unwrap changed cancellation: \{Repr(error)}")
      } noraise {
        _ => fail("cancelled stream unwrap unexpectedly completed")
      }
    })
    stream.drop()
  })
  assert_eq(resource_drop_count(3), 1)
}

///|
test "read distinguishes ownership wire and item failures" {
  reset_resource_drop_counts()
  run_agent_stream_async(async fn() {
    let (ownership_writer, ownership_stream) : (
      AgentStreamWriter[Int],
      AgentStream[Int],
    ) = AgentStream::new()
    ownership_writer.close()
    assert_true(ownership_stream.read() is None)
    try ownership_stream.read() catch {
      AgentStreamReadError::Ownership(_) => ()
      error => fail("unexpected ownership error: \{Repr(error)}")
    } noraise {
      _ => fail("read after EOF unexpectedly succeeded")
    }

    let (wire_reader, wire_sink) = @async-core.Stream::new_with_cleanup(
      @model.drop_wit_capabilities,
      capacity=1,
    )
    let malformed : @types.SchemaValueTree = {
      value_nodes: [
        @types.S32Value(1),
        @types.StreamValue(@types.SchemaValueStream::SchemaValueStream(200)),
      ],
      root: 0,
    }
    assert_true(wire_sink.write_all([malformed][:]))
    wire_sink.close()
    let wire_stream : AgentStream[Int] = AgentStream::from_reader(
      wire_reader, from_value_as,
    )
    try wire_stream.read() catch {
      AgentStreamReadError::WireDecode(_) => ()
      error => fail("unexpected wire error: \{Repr(error)}")
    } noraise {
      _ => fail("malformed wire item unexpectedly decoded")
    }

    let (item_reader, item_sink) = @async-core.Stream::new_with_cleanup(
      @model.drop_wit_capabilities,
      capacity=1,
    )
    let wrong_item : @types.SchemaValueTree = {
      value_nodes: [
        @types.StreamValue(@types.SchemaValueStream::SchemaValueStream(201)),
      ],
      root: 0,
    }
    assert_true(item_sink.write_all([wrong_item][:]))
    item_sink.close()
    let item_stream : AgentStream[Int] = AgentStream::from_reader(
      item_reader, from_value_as,
    )
    try item_stream.read() catch {
      AgentStreamReadError::ItemDecode(ShapeMismatch(..)) => ()
      error => fail("unexpected item error: \{Repr(error)}")
    } noraise {
      _ => fail("wrong typed item unexpectedly decoded")
    }

    let (partial_reader, partial_sink) = @async-core.Stream::new_with_cleanup(
      @model.drop_wit_capabilities,
      capacity=1,
    )
    let partial_item : @types.SchemaValueTree = {
      value_nodes: [
        @types.StreamValue(@types.SchemaValueStream::SchemaValueStream(202)),
        @types.StringValue("not an int"),
        @types.TupleValue([0, 1]),
      ],
      root: 2,
    }
    assert_true(partial_sink.write_all([partial_item][:]))
    partial_sink.close()
    let partial_stream : AgentStream[(AgentStream[Int], Int)] = AgentStream::from_reader(
      partial_reader, from_value_as,
    )
    try partial_stream.read() catch {
      AgentStreamReadError::ItemDecode(ShapeMismatch(..)) => ()
      error => fail("unexpected partial item error: \{Repr(error)}")
    } noraise {
      _ => fail("partially decoded item unexpectedly succeeded")
    }
  })
  assert_eq(resource_drop_count(3), 3)
}

///|
test "read after explicit drop reports ownership without a second drop" {
  reset_resource_drop_counts()
  run_agent_stream_async(async fn() {
    let stream = fake_agent_stream(210)
    stream.drop()
    try stream.read() catch {
      AgentStreamReadError::Ownership(_) => ()
      error => fail("unexpected read-after-drop error: \{Repr(error)}")
    } noraise {
      _ => fail("read after explicit drop unexpectedly succeeded")
    }
    stream.drop()
  })
  assert_eq(resource_drop_count(3), 1)
}

///|
test "unread stream transfer is affine" {
  reset_resource_drop_counts()
  run_agent_stream_async(async fn() {
    let original = fake_agent_stream(220)
    let (writer, stream) : (
      AgentStreamWriter[AgentStream[Int]],
      AgentStream[AgentStream[Int]],
    ) = AgentStream::new()
    @async-core.with_task_group(async fn(group) {
      let write = group.spawn(async fn() { writer.write_one(original) })
      let forwarded = match stream.read() {
        Some(forwarded) => forwarded
        None => fail("forwarded stream was lost")
      }
      assert_true(write.wait() is Accepted)
      forwarded.drop()
    })
    let (duplicate_writer, duplicate_stream) : (
      AgentStreamWriter[AgentStream[Int]],
      AgentStream[AgentStream[Int]],
    ) = AgentStream::new()
    try duplicate_writer.write_one(original) catch {
      @model.SchemaModelError::EncodeError(_) => ()
      error => fail("unexpected duplicate-transfer error: \{Repr(error)}")
    } noraise {
      _ => fail("duplicate stream transfer unexpectedly succeeded")
    }
    original.drop()
    duplicate_writer.close()
    duplicate_stream.drop()
    writer.close()
    assert_true(stream.read() is None)
  })
  assert_eq(resource_drop_count(3), 1)
}

///|
test "unwrap and reader failures report transport errors" {
  run_agent_stream_async(async fn() {
    defer set_schema_value_stream_host_mode(0)
    set_schema_value_stream_host_mode(1)
    let unwrap_failure = fake_agent_stream(230)
    try unwrap_failure.read() catch {
      AgentStreamReadError::Transport(_) => ()
      error => fail("unexpected unwrap failure: \{Repr(error)}")
    } noraise {
      _ => fail("failed unwrap unexpectedly produced a value")
    }

    set_schema_value_stream_host_mode(2)
    let read_failure = fake_agent_stream(231)
    try read_failure.read() catch {
      AgentStreamReadError::Transport(_) => ()
      error => fail("unexpected reader failure: \{Repr(error)}")
    } noraise {
      _ => fail("failed reader unexpectedly produced a value")
    }
    read_failure.drop()
  })
}

///|
priv struct CodecItem {
  stream : AgentStream[Int]
  value : Int
}

///|
test "fallible item codec releases prefix failing item and unconverted suffix" {
  reset_resource_drop_counts()
  let encoded = Ref(0)
  let released = Ref(0)
  run_agent_stream_async(async fn() {
    let (writer, stream) : (
      AgentStreamWriter[CodecItem],
      AgentStream[CodecItem],
    ) = AgentStream::new_with_codec(
      item => {
        encoded.val += 1
        let first = item.stream.to_schema_value()
        if item.value > 127 {
          raise FromSchemaError::Custom("original narrow integer failure")
        }
        @model.Tuple([first, @model.S8(item.value)])
      },
      _ => raise FromSchemaError::Custom("not read"),
      item => {
        released.val += 1
        item.stream.drop()
      },
    )
    let items = [
      CodecItem::{ stream: fake_agent_stream(300), value: 1, },
      CodecItem::{ stream: fake_agent_stream(301), value: 999, },
      CodecItem::{ stream: fake_agent_stream(302), value: 2, },
    ]
    try writer.write_all(items) catch {
      FromSchemaError::Custom(message) =>
        assert_eq(message, "original narrow integer failure")
      error => fail("wrong error: \{Repr(error)}")
    } noraise {
      _ => fail("expected item conversion failure")
    }
    assert_eq(encoded.val, 2)
    assert_eq(released.val, 2)
    assert_eq(resource_drop_count(3), 3)
    writer.close()
    stream.drop()
  })
}

///|
test "codec-bound producer forwards unread without running either item codec" {
  let calls = Ref(0)
  let unstarted = Ref(0)
  let stream : AgentStream[Int] = AgentStream::produce_with_codec(
    _ => calls.val += 1,
    _ => {
      calls.val += 1
      @model.S8(1)
    },
    _ => {
      calls.val += 1
      1
    },
    _ => (),
    on_unstarted_drop=() => unstarted.val += 1,
  )
  let model = stream.to_schema_value()
  let forwarded : AgentStream[Int] = AgentStream::from_value_with_codec(
    model, from_value_as,
  )
  assert_eq(calls.val, 0)
  forwarded.drop()
  stream.drop()
  assert_eq(calls.val, 0)
  assert_eq(unstarted.val, 1)
}
