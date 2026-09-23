///|
test "direct wire conversion handles nested lists, options, results, and tuples" {
  let value : Array[(String, Result[UInt64?, String])] = [
    ("first", Ok(Some(42UL))),
    ("second", Ok(None)),
    ("third", Err("failed")),
  ]
  let wire = @schema.into_wire(value)
  let decoded : Array[(String, Result[UInt64?, String])] = @schema.from_wire(
    wire,
  )
  assert_eq(decoded, value)
}

///|
test "wire invocation fields use a shared record arena rather than nested models" {
  let wire = @schema.into_wire_record(["first", 19U, Some(false)])
  assert_eq(wire.root, 4)
  assert_eq(wire.value_nodes, [
    @types.StringValue("first"),
    @types.U32Value(19U),
    @types.BoolValue(false),
    @types.OptionValue(Some(2)),
    @types.RecordValue([0, 1, 3]),
  ])
  let empty = @schema.into_wire_record([])
  assert_eq(empty.root, 0)
  assert_eq(empty.value_nodes, [@types.RecordValue([])])
}

///|
test "direct wire conversion rejects dangling indices" {
  let wire : @types.SchemaValueTree = {
    value_nodes: [@types.SchemaValueNode::OptionValue(Some(9))],
    root: 0,
  }
  try {
    let decoded : String? = @schema.from_wire(wire)
    decoded
  } catch {
    @schema.WireError::Invalid(message) =>
      assert_true(message.contains("out of bounds"))
  } noraise {
    _ => fail("expected malformed wire value to fail")
  }
}

///|
test "direct wire result accepts the payload-less unit representation" {
  let wire : @types.SchemaValueTree = {
    value_nodes: [
      @types.SchemaValueNode::ResultValue(
        @types.ResultValuePayload::OkValue(None),
      ),
    ],
    root: 0,
  }
  let decoded : Result[Unit, String] = @schema.from_wire(wire)
  assert_eq(decoded, Ok(()))
}

///|
test "direct wire schema preserves compound shape" {
  let graph = @schema.wire_schema(
    (@schema.TypeTag::{ } : @schema.TypeTag[Array[Result[String?, UInt64]]]),
  )
  assert_true(graph.type_nodes.length() == 5)
  assert_true(graph.defs.is_empty())
}

///|
test "direct wire conversion preserves supported map values" {
  let value : Map[String, UInt64] = { "first": 1UL, "second": 2UL }
  let wire = @schema.into_wire(value)
  let decoded : Map[String, UInt64] = @schema.from_wire(wire)
  assert_eq(decoded, value)
}

///|
test "unit result encoding omits the payload and its type" {
  let encoded = @schema.into_wire((Err(()) : Result[String, Unit]))
  assert_eq(encoded.value_nodes, [@types.ResultValue(@types.ErrValue(None))])
  let graph = @schema.wire_schema(
    (@schema.type_tag() : @schema.TypeTag[Result[String, Unit]]),
  )
  assert_eq(
    graph.type_nodes[graph.root].body,
    @types.ResultType({ ok: Some(0), err: None, }),
  )
  assert_eq(graph.type_nodes[0].body, @types.StringType)
}

///|
test "required result payload cannot be omitted" {
  let tree : @types.SchemaValueTree = {
    value_nodes: [@types.ResultValue(@types.OkValue(None))],
    root: 0,
  }
  try {
    let _ : Result[String, Unit] = @schema.from_wire(tree)
  } catch {
    @schema.WireError::Invalid(_) => ()
  } noraise {
    _ => fail("accepted missing string payload")
  }
}

///|
test "wire reader rejects cycles and repeated nodes" {
  for
    tree in [
      @types.SchemaValueTree::{
        value_nodes: [@types.OptionValue(Some(0))],
        root: 0,
      },
      @types.SchemaValueTree::{
        value_nodes: [@types.StringValue("x"), @types.TupleValue([0, 0])],
        root: 1,
      },
    ] {
    let reader = @schema.WireReader::new(tree)
    defer reader.abort()
    ignore(@schema.wire_node(reader, tree.root))
    if tree.root != 0 {
      ignore(@schema.wire_node(reader, 0))
    }
    try @schema.wire_node(reader, 0) catch {
      @schema.WireError::Invalid(_) => ()
    } noraise {
      _ => fail("accepted duplicate node")
    }
  }
}

///|
test "decode rollback invalidates provisional owners and drops distinct resources once" {
  let quota = @types.QuotaToken::QuotaToken(31)
  let tree : @types.SchemaValueTree = {
    value_nodes: [
      @types.QuotaTokenHandle(quota),
      @types.QuotaTokenHandle(quota),
      @types.BoolValue(true),
    ],
    root: 0,
  }
  let dropped : Array[@types.SchemaValueNode] = []
  let reader = @schema.WireReader::new(tree, drop_resource=node => {
    dropped.push(node)
  })
  let holder : @model.GuestQuotaTokenHandle = @schema.FromWire::read_wire(
    reader, 0,
  )
  try {
    let _ : String = @schema.FromWire::read_wire(reader, 2)
  } catch {
    @schema.WireError::Invalid(_) => reader.abort()
  } noraise {
    _ => fail("accepted bool as string")
  }
  reader.abort()
  assert_false(holder.is_present())
  assert_eq(dropped, [@types.QuotaTokenHandle(quota)])
}

///|
test "decode commit rejects unreachable resources without leaking any capability kind" {
  let resources = [
    @types.SecretValue(@types.Secret::Secret(1)),
    @types.QuotaTokenHandle(@types.QuotaToken::QuotaToken(2)),
    @types.PermissionCardHandle(@types.PermissionCard::PermissionCard(3)),
    @types.StreamValue(@types.SchemaValueStream::SchemaValueStream(4)),
  ]
  let dropped : Array[@types.SchemaValueNode] = []
  let tree : @types.SchemaValueTree = {
    value_nodes: [@types.TupleValue([])] + resources,
    root: 0,
  }
  let reader = @schema.WireReader::new(tree, drop_resource=node => {
    dropped.push(node)
  })
  let _ : Unit = @schema.FromWire::read_wire(reader, 0)
  try reader.finish() catch {
    @schema.WireError::Invalid(_) => ()
  } noraise {
    _ => fail("accepted unreachable resource")
  }
  reader.abort()
  assert_eq(dropped, resources)
}

///|
test "wire encoding preflights aliases before transferring any capability" {
  let secret = @model.GuestSecretHandle::new(@types.Secret::Secret(11))
  let quota = @model.GuestQuotaTokenHandle::new(
    @types.QuotaToken::QuotaToken(12),
  )
  let card = @model.GuestPermissionCardHandle::new(
    @types.PermissionCard::PermissionCard(13),
  )
  try @schema.into_wire((secret, quota, [card, card])) catch {
    @schema.WireError::Invalid(_) => ()
  } noraise {
    _ => fail("accepted aliased permission card")
  }
  assert_true(secret.is_present())
  assert_true(quota.is_present())
  assert_true(card.is_present())
  let encoded = @schema.into_wire((secret, quota, card))
  assert_false(secret.is_present())
  assert_false(quota.is_present())
  assert_false(card.is_present())
  assert_eq(
    encoded.value_nodes[0],
    @types.SecretValue(@types.Secret::Secret(11)),
  )
  assert_eq(
    encoded.value_nodes[1],
    @types.QuotaTokenHandle(@types.QuotaToken::QuotaToken(12)),
  )
  assert_eq(
    encoded.value_nodes[2],
    @types.PermissionCardHandle(@types.PermissionCard::PermissionCard(13)),
  )
}

///|
test "duplicate map keys cannot discard a lifted capability" {
  let first = @types.QuotaTokenHandle(@types.QuotaToken::QuotaToken(71))
  let second = @types.QuotaTokenHandle(@types.QuotaToken::QuotaToken(72))
  let tree : @types.SchemaValueTree = {
    value_nodes: [
      @types.StringValue("same"),
      first,
      @types.StringValue("same"),
      second,
      @types.MapValue([{ key: 0, value: 1, }, { key: 2, value: 3, }]),
    ],
    root: 4,
  }
  let dropped = []
  let reader = @schema.WireReader::new(tree, drop_resource=node => {
    dropped.push(node)
  })
  try {
    let _ : Map[String, @model.GuestQuotaTokenHandle] = @schema.FromWire::read_wire(
      reader,
      tree.root,
    )
  } catch {
    @schema.WireError::Invalid("duplicate map key") => reader.abort()
    @schema.WireError::Invalid(message) =>
      fail("unexpected wire error: \{message}")
  } noraise {
    _ => fail("duplicate map key silently discarded an owned value")
  }
  assert_eq(dropped, [first, second])
}

///|
test "wire type pool reserves recursive named definitions before their body" {
  let encoder = @schema.WireTypeEncoder::new()
  let root = encoder.named("pkg.Node", () => {
    let next = encoder.named("pkg.Node", () => {
      abort("reentered named definition")
    })
    let optional = encoder.push(@types.OptionType(next))
    encoder.push(
      @types.RecordType([
        {
          name: "next",
          body: optional,
          metadata: @schema.empty_wire_metadata(),
        },
      ]),
    )
  })
  assert_eq(encoder.defs.length(), 1)
  assert_eq(encoder.defs[0].id, "pkg.Node")
  assert_eq(encoder.defs[0].body, 2)
  assert_eq(encoder.nodes[0].body, @types.RefType(0))
  assert_eq(encoder.nodes[1].body, @types.OptionType(0))
  assert_eq(encoder.nodes[root].body, @types.RefType(0))
}
