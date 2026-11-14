///|
let resolved_reconnect : ReconnectPolicy = __RECONNECT__

///|
async fn run_recovery() -> Unit {
  let codec = public_value_codec(
    "{\"root\":{\"kind\":\"tuple\",\"value\":{\"elements\":[]}}}",
  )
  let parameters_codec = public_value_codec(
    "{\"root\":{\"kind\":\"tuple\",\"value\":{\"elements\":[{\"kind\":\"string\",\"value\":{}}]}}}",
  )
  let output_codec = public_value_codec(
    "{\"root\":{\"kind\":\"stream\",\"value\":{\"inner\":{\"kind\":\"u8\",\"value\":{}}}}}",
  )
  let resolved : ResolvedAgent = {
    configuration: {
      server: Custom("http://127.0.0.1:__PORT__", "test"),
      app_name: "app",
      env_name: "env",
      reconnect: resolved_reconnect,
    },
    agent_type_name: "RecoveryAgent",
    parameters: TupleValue([]),
    phantom_id: None,
    config: [],
    agent_id: None,
  }
  let result = invoke_streaming_agent(
    StreamEncodeContext::new(),
    codec,
    parameters_codec,
    output_codec,
    resolved,
    "run",
    TupleValue([StringValue("x".repeat(__START_TEXT_BYTES__))]),
  )
  assert_eq(result.agent_id, {
    component_id: "component",
    agent_id: "accepted-agent",
  })
  assert_eq(result.idempotency_key, "00000000-0000-0000-0000-000000000123")
  let session = stream_sessions["output"]
  let stream = open_output_stream(
    result.result.unwrap(),
    value => as_u8(value),
    u8_public_codec(),
    wire_kind=if "__MODE__" == "output-cancel-partial-packed" {
      "u8"
    } else {
      "json"
    },
  )
  assert_eq(stream.next(), Some(b'\x2a'))
  ignore(@http.get("http://127.0.0.1:__PORT__/consumed"))
  if "__MODE__" == "terminal-accounting" {
    run_terminal_accounting(session, resolved)
    return
  }
  if "__MODE__" == "output-admission-cancel" {
    run_output_admission_cancellation(session)
    return
  }
  if "__MODE__" == "output-cancel-race" ||
    "__MODE__" == "output-cancel-after-resume-head" ||
    "__MODE__" == "output-cancel-partial-packed" {
    if "__MODE__" == "output-cancel-after-resume-head" {
      for _ in 0..<2000 {
        if session.output_channels.get("output") == Some(3U) {
          break
        }
        @async.sleep(1)
      }
      assert_eq(session.output_channels.get("output"), Some(3U))
      assert_eq(session.cursor_sequences["output"], 1UL)
      assert_eq(session.output_sequences["output"], 1UL)
      assert_false(session.output_active.contains("output"))
    } else if "__MODE__" == "output-cancel-partial-packed" {
      let batch = session.output_active["output"]
      assert_eq(batch.index, 1)
      assert_eq(batch.values.length(), 3)
      assert_eq(session.cursor_sequences["output"], 0UL)
    }
    stream.cancel()
    ignore(@http.get("http://127.0.0.1:__PORT__/cancelled"))
    for _ in 0..<100 {
      if session.finished {
        break
      }
      @async.sleep(1)
    }
    assert_true(session.finished)
    if "__MODE__" == "output-cancel-after-resume-head" {
      assert_eq(session.output_sequences["output"], 1UL)
      assert_false(session.output_terminals["output"])
    } else {
      assert_eq(
        session.output_sequences["output"],
        if "__MODE__" == "output-cancel-partial-packed" {
          3UL
        } else {
          2UL
        },
      )
      assert_true(session.output_terminals["output"])
    }
    assert_eq(session.total_queue_bytes, 0)
    return
  }
  assert_eq(stream.next(), Some(b'\x2b'))
  assert_true(stream.next() is None)
}

///|
async test "streaming recovery rejects or completes" {
  connect_attempt_timeout_ms.val = 200
  stable_connection_ms.val = __STABLE__
  if "__MODE__" == "input-failure-replay" || "__MODE__" == "input-eof-replay" {
    run_input_terminal_recovery("__MODE__" == "input-failure-replay")
    return
  }
  if "__MODE__" == "input-prefetch-delayed" ||
    "__MODE__" == "input-prefetch-before-attachment" {
    run_prefetch_owner_recovery(
      "__MODE__" == "input-prefetch-before-attachment",
    )
    return
  }
  let expected = "__ERROR__"
  try run_recovery() catch {
    Unreachable(endpoint~, window_ms~, ..) => {
      assert_eq(expected, "unreachable")
      assert_true(endpoint.contains("127.0.0.1:__PORT__/"))
      assert_eq(Within(window_ms), resolved_reconnect)
    }
    BridgeError(message) => assert_eq(message, expected)
    StreamFailed(Failed(graph~, value~)) => {
      assert_eq(expected, "typed-failure")
      assert_eq(
        graph,
        parse_strict_json("{\"root\":{\"kind\":\"string\",\"value\":{}}}"),
      )
      assert_eq(
        value,
        parse_strict_json(
          "{\"kind\":\"string\",\"value\":\"application failure\"}",
        ),
      )
    }
    ConsumerCancelled(details) => {
      assert_eq(expected, "consumer-cancelled")
      assert_eq(details, Some("server consumer stopped"))
    }
    error => raise error
  } noraise {
    _ => assert_eq(expected, "")
  }
}

///|
async fn run_input_terminal_recovery(failed : Bool) -> Unit {
  let pulls = Ref(0)
  let source : AgentStream[String] = AgentStream::from_next(fn() {
    pulls.val += 1
    if pulls.val == 1 {
      Some("item")
    } else if failed {
      raise BridgeError("source diagnostic")
    } else {
      None
    }
  })
  let context = StreamEncodeContext::new()
  let reference = register_input_stream(
    context,
    source,
    fn(value) { StringValue(value) },
    public_value_codec("{\"root\":{\"kind\":\"string\",\"value\":{}}}"),
  )
  let unit_codec = public_value_codec(
    "{\"root\":{\"kind\":\"tuple\",\"value\":{\"elements\":[]}}}",
  )
  let input_codec = public_value_codec(
    "{\"root\":{\"kind\":\"stream\",\"value\":{\"inner\":{\"kind\":\"string\",\"value\":{}}}}}",
  )
  let resolved : ResolvedAgent = {
    configuration: {
      server: Custom("http://127.0.0.1:__PORT__", "test"),
      app_name: "app",
      env_name: "env",
      reconnect: resolved_reconnect,
    },
    agent_type_name: "RecoveryAgent",
    parameters: TupleValue([]),
    phantom_id: None,
    config: [],
    agent_id: None,
  }
  ignore(
    invoke_streaming_agent(
      context, unit_codec, input_codec, unit_codec, resolved, "run", reference,
    ),
  )
  assert_eq(pulls.val, 2)
  let input = context.inputs[0]
  for _ in 0..<100 {
    if input.terminal {
      break
    }
    @async.sleep(1)
  }
  assert_true(input.terminal)
  assert_eq(input.failure.diagnostic, "")
  assert_eq(input.failure.bytes, 0)
  assert_true(input.pending is None)
}

///|
fn accounting_output_message(
  channel : UInt,
  sequence : UInt64,
  outcome : Json,
) -> Map[String, Json] raise {
  expect_object(
    parse_strict_json(
      protocol_message("stream-end", [
        ("channel", json_uint(channel)),
        ("direction", Json::string("output")),
        ("sequence", Json::string(sequence.to_string())),
        ("outcome", outcome),
      ]),
    ),
  )
}

///|
fn accounting_input(
  session : LiveStreamSession,
  channel : UInt,
) -> InputRegistration raise {
  let input : InputRegistration = {
    id: "audit-\{channel}",
    next: () => Some(U8Value(42)),
    try_next: fn() raise { raise BridgeError("界🦄") },
    cancel: () => (),
    wire_kind: "u8",
    codec: u8_public_codec(),
    stream_token: None,
    channel: Some(channel),
    sequence: 0UL,
    terminal: false,
    source_ended: false,
    source_failed: false,
    failure: {
      diagnostic: "",
      bytes: 0,
      rejected: false,
      disposed: false,
      replay_bytes: session.input_replay_bytes,
    },
    pending: None,
  }
  session.inputs.push(input)
  input
}

///|
async fn run_terminal_accounting(
  session : LiveStreamSession,
  resolved : ResolvedAgent,
) -> Unit {
  for pair in [("audit-2", 5U), ("audit-3", 6U), ("oversized", 7U)] {
    let (token, channel) = pair
    session.output_channels[token] = channel
    session.output_sequences[token] = 0UL
    session.output_queues[token] = @aqueue.Queue(kind=@aqueue.Blocking(256))
  }
  let oversized = accounting_output_message(
    7U,
    0UL,
    input_failure_outcome("界".repeat(6 * 1024 * 1024)),
  )
  ignore(validate_server_stream_end(oversized))
  assert_true(
    oversized.to_json().stringify().length() < MAX_QUEUED_OUTPUT_BYTES,
  )
  try queue_output_end(session, oversized) catch {
    BridgeError(message) => assert_true(message.contains("resource-exhausted"))
    error => raise error
  } noraise {
    _ => fail("UTF8 diagnostic must exceed the per-output budget")
  }
  assert_eq(session.total_queue_bytes, 0)
  let graph = parse_strict_json("{\"root\":{\"kind\":\"string\",\"value\":{}}}")
  let value = obj([
    ("kind", Json::string("string")),
    ("value", Json::string("界".repeat(4 * 1024 * 1024))),
  ])
  let outcome = obj([
    ("kind", Json::string("failed")),
    (
      "error",
      obj([
        ("kind", Json::string("failed")),
        ("value", obj([("graph", graph), ("value", value)])),
      ]),
    ),
  ])
  assert_true(decode_stream_outcome(outcome) is Some(StreamFailed(Failed(..))))
  let first = accounting_output_message(2U, 1UL, outcome)
  let second = accounting_output_message(5U, 0UL, outcome)
  let third = accounting_output_message(6U, 0UL, outcome)
  let bytes = @utf8.encode(first.to_json().stringify()).length()
  assert_true(
    3 * first.to_json().stringify().length() < 2 * MAX_QUEUED_OUTPUT_BYTES,
  )
  assert_true(3 * bytes > 2 * MAX_QUEUED_OUTPUT_BYTES)
  queue_output_end(session, first)
  queue_output_end(session, second)
  assert_eq(session.total_queue_bytes, 2 * bytes)
  let started = Ref(false)
  let completed = Ref(false)
  @async.with_task_group(async fn(group) {
    group.spawn_bg(async fn() {
      started.val = true
      queue_output_end(session, third)
      completed.val = true
    })
    @async.sleep(10)
    assert_true(started.val)
    assert_false(completed.val)
    try
      receive_output(session, "output", session.output_queues["output"])
    catch {
      StreamFailed(Failed(graph=actual_graph, value=actual_value)) => {
        assert_eq(actual_graph, graph)
        assert_eq(actual_value, value)
      }
      error => raise error
    } noraise {
      _ => fail("expected the retained typed terminal")
    }
  })
  assert_true(completed.val)
  assert_eq(session.total_queue_bytes, 2 * bytes)
  drain_output_queue(session, "audit-2", session.output_queues["audit-2"])
  drain_output_queue(session, "audit-3", session.output_queues["audit-3"])
  assert_eq(session.total_queue_bytes, 0)
  println("typed output terminal budgets and release verified")

  let input = accounting_input(session, 4U)
  try
    retain_input_failure(input, BridgeError("界".repeat(6 * 1024 * 1024)))
  catch {
    BridgeError(message) => assert_true(message.contains("resource-exhausted"))
    error => raise error
  } noraise {
    _ => fail("UTF8 input diagnostic must exceed the replay budget")
  }
  assert_eq(input.failure.diagnostic, "")
  assert_eq(input.failure.bytes, 0)
  assert_eq(session.input_replay_bytes.val, 0)
  prepare_input(session, input)
  let diagnostic_bytes = @utf8.encode(
    input_failure_outcome(input.failure.diagnostic).stringify(),
  ).length()
  assert_true(
    diagnostic_bytes >
    input_failure_outcome(input.failure.diagnostic).stringify().length(),
  )
  assert_eq(input.failure.bytes, diagnostic_bytes)
  assert_eq(session.input_replay_bytes.val, diagnostic_bytes + 1)
  apply_input_high_water(session, input, 1UL, false)
  assert_eq(session.input_replay_bytes.val, diagnostic_bytes)
  cancel_failed_input(session, input)
  assert_eq(input.failure.diagnostic, "")
  assert_eq(input.failure.bytes, 0)
  assert_eq(input.pending.unwrap().bytes, diagnostic_bytes)
  assert_eq(session.input_replay_bytes.val, diagnostic_bytes)
  apply_input_high_water(session, input, 1UL, false)
  assert_true(input.pending is Some(_))
  apply_input_high_water(session, input, 1UL, true)
  assert_eq(session.input_replay_bytes.val, 0)
  assert_true(input.pending is None)
  println("input diagnostic prefix and terminal ACK ownership verified")

  let prefix_cancel = accounting_input(session, 8U)
  prepare_input(session, prefix_cancel)
  let cancel = expect_object(
    parse_strict_json(
      protocol_message("stream-end", [
        ("channel", json_uint(8U)),
        ("direction", Json::string("input")),
        ("sequence", Json::string("0")),
        ("outcome", obj([("kind", Json::string("consumer-cancelled"))])),
      ]),
    ),
  )
  assert_false(dispatch_server_message(session, cancel))
  assert_eq(prefix_cancel.failure.diagnostic, "")
  assert_eq(prefix_cancel.failure.bytes, 0)
  assert_true(prefix_cancel.pending is None)
  assert_eq(session.input_replay_bytes.val, 0)

  let terminal_cancel = accounting_input(session, 9U)
  retain_input_failure(terminal_cancel, BridgeError("界🦄"))
  cancel_failed_input(session, terminal_cancel)
  cancel["channel"] = json_uint(9U)
  assert_false(dispatch_server_message(session, cancel))
  assert_true(terminal_cancel.pending is None)
  assert_eq(terminal_cancel.failure.diagnostic, "")
  assert_eq(terminal_cancel.failure.bytes, 0)
  assert_eq(session.input_replay_bytes.val, 0)
  println("input diagnostic cancellation ownership verified")

  let disposed = accounting_input(session, 10U)
  prepare_input(session, disposed)
  fail_session(session, BridgeError("disposed"))
  assert_true(disposed.pending is None)
  assert_eq(disposed.failure.diagnostic, "")
  assert_eq(disposed.failure.bytes, 0)
  assert_eq(session.input_replay_bytes.val, 0)
  println("input diagnostic disposal ownership verified")
  ignore(@http.get("http://127.0.0.1:__PORT__/audit-done"))
  session.actor_owner.unwrap().close()
}

///|
async fn run_output_admission_cancellation(session : LiveStreamSession) -> Unit {
  for
    pair in [
      ("pressure-1", 20U),
      ("pressure-2", 21U),
      ("cancel-byte", 11U),
      ("cancel-item", 12U),
    ] {
    let (token, channel) = pair
    session.output_channels[token] = channel
    session.output_sequences[token] = 0UL
    session.output_queues[token] = @aqueue.Queue(
      kind=@aqueue.Blocking(MAX_QUEUED_OUTPUT_ITEMS),
    )
  }
  let payload = Bytes::make(MAX_QUEUED_OUTPUT_BYTES, b'\x2a')
  for token in ["pressure-1", "pressure-2"] {
    enqueue_output(session, token, {
      values: [BinaryValue(payload, None)],
      index: 0,
      cursor: None,
      error: None,
      bytes: payload.length(),
      next_sequence: 1UL,
      terminal: false,
    })
  }
  assert_eq(session.total_queue_bytes, 2 * MAX_QUEUED_OUTPUT_BYTES)
  let finished = obj([("kind", Json::string("finished"))])
  let completed = Ref(false)
  @async.with_timeout(2000, async fn() {
    @async.with_task_group(async fn(group) {
      group.spawn_bg(async fn() {
        queue_output_end(session, accounting_output_message(11U, 0UL, finished))
        completed.val = true
      })
      @async.sleep(10)
      assert_false(completed.val)
      assert_eq(session.queue_bytes.get("cancel-byte").unwrap_or(0), 0)
      assert_true(session.output_queues["cancel-byte"].try_get() is None)
      cancel_output(session, "cancel-byte", "cancelled")
    })
  })
  assert_true(completed.val)
  assert_eq(session.queue_bytes.get("cancel-byte").unwrap_or(0), 0)
  assert_true(session.output_queues["cancel-byte"].try_get() is None)
  assert_eq(session.total_queue_bytes, 2 * MAX_QUEUED_OUTPUT_BYTES)
  for token in ["pressure-1", "pressure-2"] {
    drain_output_queue(session, token, session.output_queues[token])
  }
  for index in 0..<MAX_QUEUED_OUTPUT_ITEMS {
    enqueue_output(session, "cancel-item", {
      values: [U8Value(42)],
      index: 0,
      cursor: None,
      error: None,
      bytes: 1,
      next_sequence: index.to_uint64() + 1UL,
      terminal: false,
    })
  }
  session.output_sequences["cancel-item"] = MAX_QUEUED_OUTPUT_ITEMS.to_uint64()
  completed.val = false
  @async.with_timeout(2000, async fn() {
    @async.with_task_group(async fn(group) {
      group.spawn_bg(async fn() {
        queue_output_end(
          session,
          accounting_output_message(
            12U,
            MAX_QUEUED_OUTPUT_ITEMS.to_uint64(),
            finished,
          ),
        )
        completed.val = true
      })
      @async.sleep(10)
      assert_false(completed.val)
      assert_eq(session.total_queue_bytes, MAX_QUEUED_OUTPUT_ITEMS)
      cancel_output(session, "cancel-item", "cancelled")
    })
  })
  assert_true(completed.val)
  assert_eq(session.total_queue_bytes, 0)
  assert_eq(session.queue_bytes["cancel-item"], 0)
  assert_true(session.output_queues["cancel-item"].try_get() is None)
  println("byte- and item-blocked terminal cancellation verified")
  fail_session(session, BridgeError("disposed"))
  ignore(@http.get("http://127.0.0.1:__PORT__/audit-done"))
  session.actor_owner.unwrap().close()
}

///|
async fn run_prefetch_owner_recovery(before_attachment : Bool) -> Unit {
  let gate = @cond.Cond()
  let pulls = Ref(0)
  let source : AgentStream[Byte] = AgentStream::from_next(async fn() {
    pulls.val += 1
    if pulls.val == 1 {
      Some(b'\x2a')
    } else {
      if !before_attachment {
        gate.wait()
      }
      raise BridgeError("delayed 界🦄 diagnostic")
    }
  })
  let context = StreamEncodeContext::new()
  let reference = register_input_stream(
    context,
    source,
    value => U8Value(value.to_int()),
    u8_public_codec(),
    wire_kind="u8",
  )
  let input = context.inputs[0]
  if before_attachment {
    for _ in 0..<1000 {
      if input.failure.bytes > 0 {
        break
      }
      @async.sleep(1)
    }
    assert_true(input.failure.bytes > 0)
    assert_eq(context.replay_bytes.val, input.failure.bytes)
    assert_false(input.source_failed)
  }
  let unit = public_value_codec(
    "{\"root\":{\"kind\":\"tuple\",\"value\":{\"elements\":[]}}}",
  )
  let input_codec = public_value_codec(
    "{\"root\":{\"kind\":\"stream\",\"value\":{\"inner\":{\"kind\":\"u8\",\"value\":{}}}}}",
  )
  let resolved : ResolvedAgent = {
    configuration: {
      server: Custom("http://127.0.0.1:__PORT__", "test"),
      app_name: "app",
      env_name: "env",
      reconnect: resolved_reconnect,
    },
    agent_type_name: "RecoveryAgent",
    parameters: TupleValue([]),
    phantom_id: None,
    config: [],
    agent_id: None,
  }
  ignore(
    invoke_streaming_agent(
      context, unit, input_codec, unit, resolved, "run", reference,
    ),
  )
  if !before_attachment {
    assert_eq(input.failure.bytes, 0)
    assert_eq(context.replay_bytes.val, 1)
    gate.broadcast()
  }
  for _ in 0..<1000 {
    if input.failure.bytes > 0 {
      break
    }
    @async.sleep(1)
  }
  assert_true(input.failure.bytes > 0)
  assert_eq(context.replay_bytes.val, input.failure.bytes + 1)
  assert_eq(input.pending.unwrap().count, 1UL)
  assert_true(input.pending.unwrap().payload is PendingU8(_))
  if !before_attachment {
    assert_false(input.source_failed)
  }
  assert_true(input.failure.diagnostic.contains("delayed 界🦄 diagnostic"))
  ignore(@http.get("http://127.0.0.1:__PORT__/diagnostic-admitted"))
  for _ in 0..<1000 {
    if input.terminal {
      break
    }
    @async.sleep(1)
  }
  assert_true(input.terminal)
  assert_eq(context.replay_bytes.val, 0)
  assert_eq(input.failure.bytes, 0)
  assert_eq(input.failure.diagnostic, "")
  assert_true(input.pending is None)
  assert_eq(pulls.val, 2)
  println(
    "prefetch owner admission, prefix ordering and terminal transfer verified",
  )
}
