///|
test "direct wire UUID preserves bits and uses the UUID schema" {
  for
    value in [
      @schema.Uuid::from_bits(0UL, 0UL),
      @schema.Uuid::from_bits(18446744073709551615UL, 9223372036854775808UL),
      @schema.Uuid::from_bits(42UL, 19UL),
    ] {
    let wire = @schema.into_wire(value)
    assert_eq(wire.root, 0)
    assert_eq(wire.value_nodes, [@types.UuidValue(value.to_wit())])
    let decoded : @schema.Uuid = @schema.from_wire(wire)
    assert_eq(decoded, value)
    let nested : Array[@schema.Uuid?] = [Some(value), None]
    let decoded_nested : Array[@schema.Uuid?] = @schema.from_wire(
      @schema.into_wire(nested),
    )
    assert_eq(decoded_nested, nested)
  }
  let graph = @schema.wire_schema(
    (@schema.type_tag() : @schema.TypeTag[@schema.Uuid]),
  )
  assert_eq(graph.type_nodes.length(), 1)
  assert_eq(graph.type_nodes[graph.root].body, @types.UuidType)
  assert_true(graph.defs.is_empty())
  try {
    let _ : @schema.Uuid = @schema.from_wire(@schema.into_wire("not a uuid"))
  } catch {
    @schema.WireError::Invalid("expected uuid value") => ()
    @schema.WireError::Invalid(message) => fail("unexpected error: \{message}")
  } noraise {
    _ => fail("accepted string as uuid")
  }
}

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
  let card = @card.adopt(@types.PermissionCard::PermissionCard(13))
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
test "duplicate map keys keep the last entry on both decode paths" {
  let tree : @types.SchemaValueTree = {
    value_nodes: [
      @types.StringValue("a"),
      @types.S32Value(1),
      @types.StringValue("b"),
      @types.S32Value(2),
      @types.StringValue("a"),
      @types.S32Value(3),
      @types.MapValue([
        { key: 0, value: 1, },
        { key: 2, value: 3, },
        { key: 4, value: 5, },
      ]),
    ],
    root: 6,
  }
  let from_wire : Map[String, Int] = @schema.from_wire(tree)
  let from_value : Map[String, Int] = @schema.FromSchema::from_value(
    @model.SchemaValue::Map([
      { key: String("a"), value: S32(1), },
      { key: String("b"), value: S32(2), },
      { key: String("a"), value: S32(3), },
    ]),
  )
  assert_eq(from_wire.to_array(), [("a", 3), ("b", 2)])
  assert_eq(from_value.to_array(), from_wire.to_array())
}

///|
test "duplicate map keys drop the discarded capability exactly once" {
  let first = @types.QuotaTokenHandle(@types.QuotaToken::QuotaToken(71))
  let second = @types.QuotaTokenHandle(@types.QuotaToken::QuotaToken(72))
  let tree : @types.SchemaValueTree = {
    value_nodes: [
      @types.StringValue("same"),
      @types.OptionValue(Some(5)),
      @types.StringValue("same"),
      @types.OptionValue(Some(6)),
      @types.MapValue([{ key: 0, value: 1, }, { key: 2, value: 3, }]),
      first,
      second,
    ],
    root: 4,
  }
  let dropped = []
  let reader = @schema.WireReader::new(tree, drop_resource=node => {
    dropped.push(node)
  })
  let decoded : Map[String, @model.GuestQuotaTokenHandle?] = @schema.FromWire::read_wire(
    reader,
    tree.root,
  )
  assert_eq(dropped, [])
  reader.finish()
  assert_eq(dropped, [first])
  assert_eq(decoded.length(), 1)
  guard decoded.get("same") is Some(Some(_)) else {
    fail("expected the last entry's capability")
  }
}

///|
test "duplicate map keys drop discarded capabilities once on abort" {
  let first = @types.QuotaTokenHandle(@types.QuotaToken::QuotaToken(81))
  let second = @types.QuotaTokenHandle(@types.QuotaToken::QuotaToken(82))
  let tree : @types.SchemaValueTree = {
    value_nodes: [
      @types.StringValue("same"),
      first,
      @types.StringValue("same"),
      second,
      @types.MapValue([{ key: 0, value: 1, }, { key: 2, value: 3, }]),
      @types.TupleValue([4]),
    ],
    root: 5,
  }
  let dropped = []
  let reader = @schema.WireReader::new(tree, drop_resource=node => {
    dropped.push(node)
  })
  try {
    let _ : (Map[String, @model.GuestQuotaTokenHandle], String) = @schema.FromWire::read_wire(
      reader,
      tree.root,
    )
  } catch {
    @schema.WireError::Invalid(_) => reader.abort()
  } noraise {
    _ => fail("accepted a tuple with a missing element")
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

///|
test "wire resource identities include the resource kind" {
  let secret = @model.GuestSecretHandle::new(@types.Secret::Secret(91))
  let quota = @model.GuestQuotaTokenHandle::new(
    @types.QuotaToken::QuotaToken(91),
  )
  let card = @card.adopt(@types.PermissionCard::PermissionCard(91))
  let stream = @model.GuestSchemaValueStreamHandle::from_wrapped(
    @types.SchemaValueStream::SchemaValueStream(91),
  )
  let value = @schema.into_wire((secret, quota, card, stream))
  assert_eq(value.value_nodes.length(), 5)
  assert_false(secret.is_present())
  assert_false(quota.is_present())
  assert_false(card.is_present())
  assert_false(stream.is_present())
}

///|
test "early and late wrapped endpoint aliases preserve every holder" {
  for at in [1, 49] {
    let handles = Array::makei(50, i => {
      @model.GuestSchemaValueStreamHandle::from_wrapped(
        @types.SchemaValueStream::SchemaValueStream(100 + i),
      )
    })
    handles[at] = @model.GuestSchemaValueStreamHandle::from_wrapped(
      @types.SchemaValueStream::SchemaValueStream(100),
    )
    try @schema.into_wire(handles) catch {
      @schema.WireError::Invalid(_) => ()
    } noraise {
      _ => fail("accepted distinct holders of the same endpoint")
    }
    assert_true(handles.all(handle => handle.is_present()))
  }
}

///|
fn utf16(units : Array[Int]) -> String {
  String::from_array(units.map(unit => unit.unsafe_to_char()))
}

///|
test "unpaired surrogates are rejected on both encode paths" {
  let lone = [
    utf16([0xD800]),
    utf16([0x61, 0xDC00]),
    utf16([0xD83D, 0x61]),
    utf16([0x61, 0xDBFF]),
  ]
  for s in lone {
    let wire_values : Array[&@schema.IntoWire] = [
      s,
      @schema.Path::new(s),
      @schema.Url::new(s),
    ]
    for value in wire_values {
      try {
        let _ = @schema.into_wire_record([value])
      } catch {
        @schema.WireError::Invalid(message) =>
          assert_true(message.contains("unpaired surrogate"))
      } noraise {
        _ => fail("wire encode accepted an unpaired surrogate")
      }
    }
    for
      value in [
        @model.SchemaValue::String(s),
        @model.SchemaValue::Path(s),
        @model.SchemaValue::Url(s),
        @model.SchemaValue::Text(s, None),
        @model.SchemaValue::List([String("ok"), String(s)]),
      ] {
      try {
        let _ = @model.schema_value_to_wit(value)
      } catch {
        @model.SchemaModelError::EncodeError(message) =>
          assert_true(message.contains("unpaired surrogate"))
        other => fail("unexpected error: \{repr(other)}")
      } noraise {
        _ => fail("schema value encode accepted an unpaired surrogate")
      }
    }
  }
  let emoji = utf16([0x61, 0xD83D, 0xDE00])
  assert_eq(emoji, "a😀")
  let decoded : String = @schema.from_wire(@schema.into_wire(emoji))
  assert_eq(decoded, emoji)
  let tree = @model.schema_value_to_wit(@schema.IntoSchema::to_value(emoji))
  assert_eq(tree.value_nodes, [@types.StringValue(emoji)])
}

///|
test "surrogate chars are rejected on both encode paths" {
  let surrogate = (0xDFFF).unsafe_to_char()
  try {
    let _ = @schema.into_wire(surrogate)
  } catch {
    @schema.WireError::Invalid(message) =>
      assert_true(message.contains("Unicode scalar value"))
  } noraise {
    _ => fail("wire encode accepted a surrogate char")
  }
  try {
    let _ = @model.schema_value_to_wit(@schema.IntoSchema::to_value(surrogate))
  } catch {
    @model.SchemaModelError::EncodeError(message) =>
      assert_true(message.contains("Unicode scalar value"))
    other => fail("unexpected error: \{repr(other)}")
  } noraise {
    _ => fail("schema value encode accepted a surrogate char")
  }
  let decoded : Char = @schema.from_wire(@schema.into_wire('😀'))
  assert_eq(decoded, '😀')
}

///|
test "unit result arms reject a present payload on both decode paths" {
  let unit_payload : @types.SchemaValueTree = {
    value_nodes: [
      @types.TupleValue([]),
      @types.ResultValue(@types.OkValue(Some(0))),
    ],
    root: 1,
  }
  try {
    let _ : Result[Unit, String] = @schema.from_wire(unit_payload)
  } catch {
    @schema.WireError::Invalid(message) =>
      assert_eq(message, "unexpected payload")
  } noraise {
    _ => fail("accepted a payload for a unit ok arm")
  }
  let unit_err_payload : @types.SchemaValueTree = {
    value_nodes: [
      @types.TupleValue([]),
      @types.ResultValue(@types.ErrValue(Some(0))),
    ],
    root: 1,
  }
  try {
    let _ : Result[String, Unit] = @schema.from_wire(unit_err_payload)
  } catch {
    @schema.WireError::Invalid(message) =>
      assert_eq(message, "unexpected payload")
  } noraise {
    _ => fail("accepted a payload for a unit err arm")
  }
  for
    value in [
      @model.SchemaValue::ResultOk(Some(Tuple([]))),
      @model.SchemaValue::ResultErr(Some(Tuple([]))),
    ] {
    try {
      let _ : Result[Unit, Unit] = @schema.FromSchema::from_value(value)
    } catch {
      @schema.FromSchemaError::Custom(message) =>
        assert_eq(message, "unexpected payload for a unit result arm")
      other => fail("unexpected error: \{repr(other)}")
    } noraise {
      _ => fail("accepted a payload for a unit result arm")
    }
  }
  let ok : Result[Unit, Unit] = @schema.FromSchema::from_value(ResultOk(None))
  assert_eq(ok, Ok(()))
  let err : Result[Unit, Unit] = @schema.FromSchema::from_value(ResultErr(None))
  assert_eq(err, Err(()))
  let wire_ok : Result[Unit, String] = @schema.from_wire(
    @schema.into_wire((Ok(()) : Result[Unit, String])),
  )
  assert_eq(wire_ok, Ok(()))
}

///|
test "non-unit result arms reject a missing payload on both decode paths" {
  for
    tree in [
      (
        { value_nodes: [@types.ResultValue(@types.OkValue(None))], root: 0, } :
        @types.SchemaValueTree),
      { value_nodes: [@types.ResultValue(@types.ErrValue(None))], root: 0, },
    ] {
    try {
      let _ : Result[String, String] = @schema.from_wire(tree)
    } catch {
      @schema.WireError::Invalid(message) =>
        assert_eq(message, "missing required payload")
    } noraise {
      _ => fail("accepted a missing payload for a non-unit arm")
    }
  }
  for
    value in [
      @model.SchemaValue::ResultOk(None),
      @model.SchemaValue::ResultErr(None),
    ] {
    try {
      let _ : Result[String, String] = @schema.FromSchema::from_value(value)
    } catch {
      @schema.FromSchemaError::Custom(message) =>
        assert_eq(message, "missing payload for a result arm")
      other => fail("unexpected error: \{repr(other)}")
    } noraise {
      _ => fail("accepted a missing payload for a non-unit arm")
    }
  }
}
