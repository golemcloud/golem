///|
#warnings("-struct_never_constructed")
struct EnglishOrGerman {}

///|
impl @schema.AllowedLanguages for EnglishOrGerman with fn languages() {
  ["en", "de"]
}

///|
#warnings("-struct_never_constructed")
struct ImageMimeTypes {}

///|
impl @schema.AllowedMimeTypes for ImageMimeTypes with fn mime_types() {
  ["image/png", "image/jpeg"]
}

///|
type LocalizedText = @schema.UnstructuredText[EnglishOrGerman]

///|
type Image = @schema.UnstructuredBinary[ImageMimeTypes]

///|
type AnyBinary = @schema.UnstructuredBinary[@schema.AnyMimeType]

///|
fn[T : @schema.WireSchema] wire_graph_of(
  tag : @schema.TypeTag[T],
) -> @types.SchemaGraph {
  @schema.wire_schema(tag)
}

///|
fn[T : @schema.IntoSchema] model_graph_of(
  tag : @schema.TypeTag[T],
) -> @model.SchemaGraph raise {
  @schema.into_schema_graph(tag)
}

///|
fn[T : @schema.IntoWire + @schema.FromWire] wire_roundtrip(
  value : T,
) -> T raise {
  @schema.from_wire(@schema.into_wire(value))
}

///|
fn[T : @schema.IntoSchema + @schema.FromSchema] model_roundtrip(
  value : T,
) -> T raise {
  let typed = @schema.try_into_typed_schema_value(value)
  let wit = @model.typed_schema_value_to_wit(typed)
  let back = @model.typed_schema_value_from_wit(wit, {
    drop_secret: fn(_) {  },
    drop_quota_token: fn(_) {  },
    drop_stream: fn(_) {  },
    drop_native_stream: fn(_) {  },
  })
  assert_eq(back.graph, typed.graph)
  @schema.from_value_as(back.value)
}

///|
test "unstructured text wire schema is the role-marked inline/url variant" {
  debug_inspect(
    wire_graph_of((@schema.type_tag() : @schema.TypeTag[LocalizedText])),
    content=(
      #|{
      #|  type_nodes: [
      #|    {
      #|      body: TextType(
      #|        {
      #|          languages: Some(["en", "de"]),
      #|          min_length: None,
      #|          max_length: None,
      #|          regex: None,
      #|        },
      #|      ),
      #|      metadata: { doc: None, aliases: [], examples: [], deprecated: None, role: None },
      #|    },
      #|    {
      #|      body: UrlType({ allowed_schemes: None, allowed_hosts: None }),
      #|      metadata: { doc: None, aliases: [], examples: [], deprecated: None, role: None },
      #|    },
      #|    {
      #|      body: VariantType(
      #|        [
      #|          {
      #|            name: "inline",
      #|            payload: Some(0),
      #|            metadata: { doc: None, aliases: [], examples: [], deprecated: None, role: None },
      #|          },
      #|          {
      #|            name: "url",
      #|            payload: Some(1),
      #|            metadata: { doc: None, aliases: [], examples: [], deprecated: None, role: None },
      #|          },
      #|        ],
      #|      ),
      #|      metadata: {
      #|        doc: None,
      #|        aliases: [],
      #|        examples: [],
      #|        deprecated: None,
      #|        role: Some(UnstructuredText),
      #|      },
      #|    },
      #|  ],
      #|  defs: [],
      #|  root: 2,
      #|}
    ),
  )
}

///|
test "unstructured binary wire schema carries the MIME type restrictions" {
  let graph = wire_graph_of((@schema.type_tag() : @schema.TypeTag[Image]))
  assert_eq(graph.defs, [])
  assert_eq(graph.root, 2)
  assert_eq(
    graph.type_nodes[0].body,
    @types.BinaryType({
      mime_types: Some(["image/png", "image/jpeg"]),
      min_bytes: None,
      max_bytes: None,
    }),
  )
  assert_eq(
    graph.type_nodes[1].body,
    @types.UrlType({ allowed_schemes: None, allowed_hosts: None, }),
  )
  guard graph.type_nodes[2].body is @types.VariantType([inline, url]) else {
    fail("expected a two-case variant")
  }
  assert_eq((inline.name, inline.payload), ("inline", Some(0)))
  assert_eq((url.name, url.payload), ("url", Some(1)))
  assert_eq(graph.type_nodes[2].metadata.role, Some(@types.UnstructuredBinary))
  let unrestricted = wire_graph_of(
    (@schema.type_tag() : @schema.TypeTag[AnyBinary]),
  )
  assert_eq(
    unrestricted.type_nodes[0].body,
    @types.BinaryType({ mime_types: None, min_bytes: None, max_bytes: None, }),
  )
  let any_text = wire_graph_of(
    (
      @schema.type_tag() :
      @schema.TypeTag[@schema.UnstructuredText[@schema.AnyLanguage]]),
  )
  assert_eq(
    any_text.type_nodes[0].body,
    @types.TextType({
      languages: None,
      min_length: None,
      max_length: None,
      regex: None,
    }),
  )
}

///|
test "unstructured model schema matches the wire schema" {
  assert_eq(
    @model.schema_graph_to_wit(
      model_graph_of((@schema.type_tag() : @schema.TypeTag[LocalizedText])),
    ),
    wire_graph_of((@schema.type_tag() : @schema.TypeTag[LocalizedText])),
  )
  assert_eq(
    @model.schema_graph_to_wit(
      model_graph_of((@schema.type_tag() : @schema.TypeTag[Image])),
    ),
    wire_graph_of((@schema.type_tag() : @schema.TypeTag[Image])),
  )
  let root = model_graph_of((@schema.type_tag() : @schema.TypeTag[Image])).root
  assert_eq(root.metadata.role, Some(@types.UnstructuredBinary))
  guard root.body is Variant([inline, url]) else {
    fail("expected a two-case variant")
  }
  assert_eq(inline.name, "inline")
  assert_true(inline.payload is Some({ body: Binary(_), .. }))
  assert_eq(url.name, "url")
  assert_true(url.payload is Some({ body: Url(_), .. }))
}

///|
test "unstructured wrappers nest inside option and result without named defs" {
  let graph = wire_graph_of(
    (@schema.type_tag() : @schema.TypeTag[Result[Image?, String]]),
  )
  assert_eq(graph.defs, [])
  let roles = graph.type_nodes.filter_map(node => node.metadata.role)
  assert_eq(roles, [@types.UnstructuredBinary])
  guard graph.type_nodes[graph.root].body
    is @types.ResultType({ ok: Some(ok), err: Some(_), }) else {
    fail("expected a result root")
  }
  guard graph.type_nodes[ok].body is @types.OptionType(inner) else {
    fail("expected an option ok type")
  }
  assert_eq(
    graph.type_nodes[inner].metadata.role,
    Some(@types.UnstructuredBinary),
  )
  let model = model_graph_of(
    (@schema.type_tag() : @schema.TypeTag[Result[LocalizedText?, String]]),
  )
  assert_eq(model.defs, [])
  guard model.root.body is Result(Some(ok), Some(_)) && ok.body is Option(inner) else {
    fail("expected result<option<_>, _>")
  }
  assert_eq(inner.metadata.role, Some(@types.UnstructuredText))
}

///|
test "unstructured wire values use inline case 0 and url case 1" {
  let inline : Image = @schema.UnstructuredBinary::inline(
    b"\x89PNG",
    mime_type="image/png",
  )
  assert_eq(@schema.into_wire(inline), {
    value_nodes: [
      @types.BinaryValue({
        bytes: [0x89, b'P', b'N', b'G'],
        mime_type: Some("image/png"),
      }),
      @types.VariantValue({ case: 0, payload: Some(0), }),
    ],
    root: 1,
  })
  let url : LocalizedText = @schema.UnstructuredText::url(
    "https://example.com/a.txt",
  )
  assert_eq(@schema.into_wire(url), {
    value_nodes: [
      @types.UrlValue("https://example.com/a.txt"),
      @types.VariantValue({ case: 1, payload: Some(0), }),
    ],
    root: 1,
  })
  let text : LocalizedText = @schema.UnstructuredText::inline(
    "hallo",
    language="de",
  )
  assert_eq(@schema.into_wire(text), {
    value_nodes: [
      @types.TextValue({ text: "hallo", language: Some("de"), }),
      @types.VariantValue({ case: 0, payload: Some(0), }),
    ],
    root: 1,
  })
}

///|
test "unstructured values round-trip through the wire and schema model" {
  let texts : Array[LocalizedText] = [
    Inline(text="hello", language=Some("en")),
    Inline(text="no language", language=None),
    Url("https://example.com/doc.txt"),
  ]
  let binaries : Array[Image] = [
    Inline(bytes=b"\x00\x01\x02", mime_type=Some("image/jpeg")),
    Inline(bytes=b"", mime_type=None),
    Url("https://example.com/image.png"),
  ]
  for value in texts {
    assert_eq(wire_roundtrip(value), value)
    assert_eq(model_roundtrip(value), value)
  }
  for value in binaries {
    assert_eq(wire_roundtrip(value), value)
    assert_eq(model_roundtrip(value), value)
  }
  let nested : Array[Result[Image?, String]] = [
    Ok(Some(binaries[0])),
    Ok(Some(binaries[2])),
    Ok(None),
    Err("failed"),
  ]
  for value in nested {
    assert_eq(wire_roundtrip(value), value)
    assert_eq(model_roundtrip(value), value)
  }
}

///|
test "unstructured decoders accept bare text and binary values" {
  let text : LocalizedText = @schema.from_wire({
    value_nodes: [@types.TextValue({ text: "bare", language: Some("en"), })],
    root: 0,
  })
  assert_eq(text, Inline(text="bare", language=Some("en")))
  let binary : AnyBinary = @schema.from_wire({
    value_nodes: [@types.BinaryValue({ bytes: [1, 2], mime_type: None, })],
    root: 0,
  })
  assert_eq(binary, Inline(bytes=b"\x01\x02", mime_type=None))
  let nested : Result[AnyBinary?, String] = @schema.from_wire({
    value_nodes: [
      @types.BinaryValue({ bytes: [7], mime_type: Some("application/x"), }),
      @types.OptionValue(Some(0)),
      @types.ResultValue(@types.OkValue(Some(1))),
    ],
    root: 2,
  })
  assert_eq(
    nested,
    Ok(Some(Inline(bytes=b"\x07", mime_type=Some("application/x")))),
  )
  let model_text : LocalizedText = @schema.from_value_as(
    @model.SchemaValue::Text("bare", None),
  )
  assert_eq(model_text, Inline(text="bare", language=None))
  let model_binary : Image = @schema.from_value_as(
    @model.SchemaValue::Binary([1], Some("image/png")),
  )
  assert_eq(model_binary, Inline(bytes=b"\x01", mime_type=Some("image/png")))
}

///|
test "unstructured decoders reject values outside the restrictions" {
  try {
    let _ : LocalizedText = @schema.from_wire({
      value_nodes: [
        @types.TextValue({ text: "bonjour", language: Some("fr"), }),
        @types.VariantValue({ case: 0, payload: Some(0), }),
      ],
      root: 1,
    })
  } catch {
    err =>
      debug_inspect(
        err,
        content=(
          #|Invalid("UnstructuredText: language code 'fr' is not allowed; allowed: en, de")
        ),
      )
  } noraise {
    _ => fail("accepted a disallowed language")
  }
  try {
    let _ : Image = @schema.from_wire({
      value_nodes: [
        @types.BinaryValue({ bytes: [], mime_type: Some("text/plain"), }),
      ],
      root: 0,
    })
  } catch {
    err =>
      debug_inspect(
        err,
        content=(
          #|Invalid("UnstructuredBinary: MIME type 'text/plain' is not allowed; allowed: image/png, image/jpeg")
        ),
      )
  } noraise {
    _ => fail("accepted a disallowed MIME type")
  }
  try {
    let _ : Image = @schema.from_value_as(
      @model.SchemaValue::Variant(0, Some(Binary([], Some("text/plain")))),
    )
  } catch {
    err =>
      debug_inspect(
        err,
        content=(
          #|Custom("MIME type 'text/plain' is not allowed; allowed: image/png, image/jpeg")
        ),
      )
  } noraise {
    _ => fail("accepted a disallowed MIME type")
  }
  try {
    let _ : LocalizedText = @schema.from_wire({
      value_nodes: [
        @types.UrlValue("https://example.com"),
        @types.VariantValue({ case: 0, payload: Some(0), }),
      ],
      root: 1,
    })
  } catch {
    @schema.WireError::Invalid(message) =>
      inspect(
        message,
        content="UnstructuredText: expected inline text or url payload",
      )
  } noraise {
    _ => fail("accepted a url payload in the inline case")
  }
}
