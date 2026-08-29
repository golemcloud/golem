///|
/// Literals emitted by the tool generator. Scalar tokens are interpreted from
/// their authored spelling according to the target schema.
pub(all) enum ToolLiteral {
  Scalar(String)
  List(Array[String])
  Map(Array[(String, String)])
} derive(Eq, Debug)

///|
pub fn literal_to_schema_value(
  graph : @model.SchemaGraph,
  literal : ToolLiteral,
) -> @model.SchemaValue raise ToolBuildError {
  interpret_literal(graph, graph.root, literal)
}

///|
pub fn value_is_literal_to_schema_value(
  graph : @model.SchemaGraph,
  literal : ToolLiteral,
) -> @model.SchemaValue raise ToolBuildError {
  literal_to_schema_value(graph, literal) catch {
    direct => {
      let root = resolve_literal_type(graph, graph.root)
      let root = peel_options(graph, root)
      let item = match root.body {
        @model.SchemaTypeBody::List(item) => Some(item)
        @model.SchemaTypeBody::FixedList(item, _) => Some(item)
        @model.SchemaTypeBody::Map(_, value) => Some(value)
        _ => None
      }
      match item {
        Some(item) =>
          interpret_literal(graph, item, literal) catch {
            _ => raise direct
          }
        None => raise direct
      }
    }
  }
}

///|
fn resolve_literal_type(
  graph : @model.SchemaGraph,
  type_ : @model.SchemaType,
) -> @model.SchemaType raise ToolBuildError {
  match type_.body {
    @model.SchemaTypeBody::Ref(id) =>
      for def in graph.defs {
        if def.id == id {
          return resolve_literal_type(graph, def.body)
        }
      } nobreak {
        raise LiteralTypeMismatch("unknown schema reference '\{id}'")
      }
    _ => type_
  }
}

///|
fn peel_options(
  graph : @model.SchemaGraph,
  type_ : @model.SchemaType,
) -> @model.SchemaType raise ToolBuildError {
  match type_.body {
    @model.SchemaTypeBody::Option(inner) =>
      peel_options(graph, resolve_literal_type(graph, inner))
    _ => type_
  }
}

///|
fn literal_mismatch(
  type_ : @model.SchemaType,
  literal : ToolLiteral,
) -> ToolBuildError {
  LiteralTypeMismatch(
    "literal \{to_repr(literal)} is not valid for schema type \{type_.body}",
  )
}

///|
fn[T : @strconv.FromStr] parse_authored(
  token : String,
  expected : String,
) -> T raise ToolBuildError {
  @strconv.from_str(token) catch {
    _ => raise InvalidLiteral("invalid \{expected} literal '\{token}'")
  }
}

///|
fn parse_char(token : String) -> Char raise ToolBuildError {
  let chars = token.to_array()
  guard chars is [char] else {
    raise InvalidLiteral(
      "invalid char literal '\{token}': expected one character",
    )
  }
  char
}

///|
fn interpret_scalar(
  graph : @model.SchemaGraph,
  type_ : @model.SchemaType,
  token : String,
) -> @model.SchemaValue raise ToolBuildError {
  let resolved = resolve_literal_type(graph, type_)
  match resolved.body {
    @model.SchemaTypeBody::Bool =>
      @model.SchemaValue::Bool(parse_authored(token, "bool"))
    @model.SchemaTypeBody::S8(_) => {
      let value : Int = parse_authored(token, "s8")
      guard value >= -128 && value <= 127 else {
        raise InvalidLiteral("s8 literal '\{token}' is out of range")
      }
      @model.SchemaValue::S8(value)
    }
    @model.SchemaTypeBody::S16(_) => {
      let value : Int = parse_authored(token, "s16")
      guard value >= -32768 && value <= 32767 else {
        raise InvalidLiteral("s16 literal '\{token}' is out of range")
      }
      @model.SchemaValue::S16(value)
    }
    @model.SchemaTypeBody::S32(_) =>
      @model.SchemaValue::S32(parse_authored(token, "s32"))
    @model.SchemaTypeBody::S64(_) =>
      @model.SchemaValue::S64(parse_authored(token, "s64"))
    @model.SchemaTypeBody::U8(_) => {
      let value : UInt = parse_authored(token, "u8")
      guard value <= 255 else {
        raise InvalidLiteral("u8 literal '\{token}' is out of range")
      }
      @model.SchemaValue::U8(value.to_byte())
    }
    @model.SchemaTypeBody::U16(_) =>
      @model.SchemaValue::U16(parse_authored(token, "u16"))
    @model.SchemaTypeBody::U32(_) =>
      @model.SchemaValue::U32(parse_authored(token, "u32"))
    @model.SchemaTypeBody::U64(_) =>
      @model.SchemaValue::U64(parse_authored(token, "u64"))
    @model.SchemaTypeBody::F32(_) => {
      let value : Double = parse_authored(token, "f32")
      @model.SchemaValue::F32(Float::from_double(value))
    }
    @model.SchemaTypeBody::F64(_) =>
      @model.SchemaValue::F64(parse_authored(token, "f64"))
    @model.SchemaTypeBody::Char => @model.SchemaValue::Char(parse_char(token))
    @model.SchemaTypeBody::String => @model.SchemaValue::String(token)
    @model.SchemaTypeBody::Text(_) => @model.SchemaValue::Text(token, None)
    @model.SchemaTypeBody::Path(_) => @model.SchemaValue::Path(token)
    @model.SchemaTypeBody::Url(_) => @model.SchemaValue::Url(token)
    @model.SchemaTypeBody::Enum(cases) =>
      match cases.find_index(case => case == token) {
        Some(index) => @model.SchemaValue::Enum(index.to_uint())
        None =>
          raise InvalidLiteral(
            "enum case '\{token}' is not one of [\{cases.join(", ")}]",
          )
      }
    @model.SchemaTypeBody::Option(inner) =>
      @model.SchemaValue::Option(Some(interpret_scalar(graph, inner, token)))
    _ => raise literal_mismatch(resolved, ToolLiteral::Scalar(token))
  }
}

///|
fn interpret_literal(
  graph : @model.SchemaGraph,
  type_ : @model.SchemaType,
  literal : ToolLiteral,
) -> @model.SchemaValue raise ToolBuildError {
  let resolved = resolve_literal_type(graph, type_)
  match (resolved.body, literal) {
    (_, ToolLiteral::Scalar(token)) => interpret_scalar(graph, resolved, token)
    (@model.SchemaTypeBody::Option(inner), literal) =>
      @model.SchemaValue::Option(Some(interpret_literal(graph, inner, literal)))
    (@model.SchemaTypeBody::List(item), ToolLiteral::List(tokens)) =>
      @model.SchemaValue::List(
        tokens.map(token => interpret_scalar(graph, item, token)),
      )
    (@model.SchemaTypeBody::FixedList(item, length), ToolLiteral::List(tokens)) => {
      guard tokens.length().to_uint() == length else {
        raise InvalidLiteral(
          "fixed-list literal has \{tokens.length()} items, expected \{length}",
        )
      }
      @model.SchemaValue::FixedList(
        tokens.map(token => interpret_scalar(graph, item, token)),
      )
    }
    (@model.SchemaTypeBody::Map(key, value), ToolLiteral::Map(entries)) =>
      @model.SchemaValue::Map(
        entries.map(entry => @model.SchemaMapEntry::{
          key: interpret_scalar(graph, key, entry.0),
          value: interpret_scalar(graph, value, entry.1),
        }),
      )
    (@model.SchemaTypeBody::Map(_, _), ToolLiteral::List([])) =>
      @model.SchemaValue::Map([])
    _ => raise literal_mismatch(resolved, literal)
  }
}
