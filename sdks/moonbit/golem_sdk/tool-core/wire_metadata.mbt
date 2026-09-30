///|
/// A type index in the shared pool used by a generated tool descriptor.
pub(all) struct WireToolType {
  encoder : @schema.WireTypeEncoder
  index : Int
}

///|
pub fn[T : @schema.WireSchema] wire_tool_type(
  encoder : @schema.WireTypeEncoder,
  tag : @schema.TypeTag[T],
) -> WireToolType {
  { encoder, index: @schema.append_wire_type_with(encoder, tag), }
}

///|
fn WireToolType::resolved(
  self : WireToolType,
) -> @types.SchemaTypeNode raise ToolBuildError {
  for index = self.index, remaining = self.encoder.nodes.length(); remaining > 0; {
    let node = self.encoder.nodes
      .get(index)
      .unwrap_or_else(() => abort("generated type index"))
    match node.body {
      @types.RefType(definition) => {
        guard self.encoder.defs.get(definition) is Some(def) else {
          raise SchemaModelError("unknown wire schema definition")
        }
        continue def.body, remaining - 1
      }
      _ => return node
    }
  } nobreak {
    raise SchemaModelError("cyclic wire schema definition")
  }
}

///|
fn WireToolType::with_body(
  self : WireToolType,
  body : @types.SchemaTypeBody,
) -> WireToolType raise ToolBuildError {
  let metadata = self.resolved().metadata
  let index = self.encoder.nodes.length()
  self.encoder.nodes.push({ body, metadata, })
  { encoder: self.encoder, index, }
}

///|
pub fn wire_refine_text(
  type_ : WireToolType,
  regex : String?,
  min_length : UInt?,
  max_length : UInt?,
) -> WireToolType raise ToolBuildError {
  let current : @types.TextRestrictions = match type_.resolved().body {
    @types.StringType =>
      { languages: None, min_length: None, max_length: None, regex: None, }
    @types.TextType(restrictions) => restrictions
    _ => raise RefinementTypeMismatch("text", "non-text wire type")
  }
  let min_length = overlay(min_length, current.min_length)
  let max_length = overlay(max_length, current.max_length)
  if min_length is Some(min) && max_length is Some(max) && min > max {
    raise InvalidLiteral("text minimum length exceeds maximum")
  }
  type_.with_body(
    @types.TextType({
      languages: current.languages,
      min_length,
      max_length,
      regex: overlay(regex, current.regex),
    }),
  )
}

///|
pub fn wire_refine_path(
  type_ : WireToolType,
  direction : @types.PathDirection?,
  kind : @types.PathKind?,
  mime_types : Array[String]?,
) -> WireToolType raise ToolBuildError {
  guard type_.resolved().body is @types.PathType(spec) else {
    raise RefinementTypeMismatch("path", "non-path wire type")
  }
  type_.with_body(
    @types.PathType({
      direction: direction.unwrap_or(spec.direction),
      kind: kind.unwrap_or(spec.kind),
      allowed_mime_types: overlay(mime_types, spec.allowed_mime_types),
      allowed_extensions: spec.allowed_extensions,
    }),
  )
}

///|
pub fn wire_refine_url(
  type_ : WireToolType,
  schemes : Array[String]?,
) -> WireToolType raise ToolBuildError {
  guard type_.resolved().body is @types.UrlType(spec) else {
    raise RefinementTypeMismatch("url", "non-url wire type")
  }
  type_.with_body(
    @types.UrlType({
      allowed_schemes: overlay(schemes, spec.allowed_schemes),
      allowed_hosts: spec.allowed_hosts,
    }),
  )
}

///|
fn wire_numeric_bound(
  token : String,
  body : @types.SchemaTypeBody,
) -> @types.NumericBound raise ToolBuildError {
  match body {
    @types.S8Type(_)
    | @types.S16Type(_)
    | @types.S32Type(_)
    | @types.S64Type(_) => {
      let value : Int64 = parse_authored(token, "signed bound")
      let valid = match body {
        @types.S8Type(_) => value >= -128L && value <= 127L
        @types.S16Type(_) => value >= -32768L && value <= 32767L
        @types.S32Type(_) => value >= -2147483648L && value <= 2147483647L
        _ => true
      }
      guard valid else {
        raise InvalidNumericBound("signed bound out of range")
      }
      @types.Signed(value)
    }
    @types.U8Type(_)
    | @types.U16Type(_)
    | @types.U32Type(_)
    | @types.U64Type(_) => {
      let value : UInt64 = parse_authored(token, "unsigned bound")
      let valid = match body {
        @types.U8Type(_) => value <= 255UL
        @types.U16Type(_) => value <= 65535UL
        @types.U32Type(_) => value <= 4294967295UL
        _ => true
      }
      guard valid else {
        raise InvalidNumericBound("unsigned bound out of range")
      }
      @types.Unsigned(value)
    }
    @types.F32Type(_) | @types.F64Type(_) => {
      let value : Double = parse_authored(token, "floating-point bound")
      guard !value.is_nan() && !value.is_inf() else {
        raise InvalidNumericBound("floating-point bound must be finite")
      }
      @types.FloatBits(
        if value == 0.0 {
          0UL
        } else {
          value.reinterpret_as_uint64()
        },
      )
    }
    _ => raise RefinementTypeMismatch("numeric", "non-numeric wire type")
  }
}

///|
pub fn wire_refine_numeric(
  type_ : WireToolType,
  min : String?,
  max : String?,
  unit : String?,
) -> WireToolType raise ToolBuildError {
  let body = type_.resolved().body
  let existing = match body {
    @types.S8Type(r)
    | @types.S16Type(r)
    | @types.S32Type(r)
    | @types.S64Type(r)
    | @types.U8Type(r)
    | @types.U16Type(r)
    | @types.U32Type(r)
    | @types.U64Type(r)
    | @types.F32Type(r)
    | @types.F64Type(r) => r
    _ => raise RefinementTypeMismatch("numeric", "non-numeric wire type")
  }
  let current : @types.NumericRestrictions = existing.unwrap_or({
    min: None,
    max: None,
    unit: None,
  })
  let restrictions : @types.NumericRestrictions = {
    min: overlay(min.map(token => wire_numeric_bound(token, body)), current.min),
    max: overlay(max.map(token => wire_numeric_bound(token, body)), current.max),
    unit: overlay(unit, current.unit),
  }
  if restrictions.min is Some(lo) &&
    restrictions.max is Some(hi) &&
    bound_greater(lo, hi) {
    raise InvalidNumericBound("numeric minimum exceeds maximum")
  }
  let r = if restrictions.min is None &&
    restrictions.max is None &&
    restrictions.unit is None {
    None
  } else {
    Some(restrictions)
  }
  type_.with_body(
    match body {
      @types.S8Type(_) => @types.S8Type(r)
      @types.S16Type(_) => @types.S16Type(r)
      @types.S32Type(_) => @types.S32Type(r)
      @types.S64Type(_) => @types.S64Type(r)
      @types.U8Type(_) => @types.U8Type(r)
      @types.U16Type(_) => @types.U16Type(r)
      @types.U32Type(_) => @types.U32Type(r)
      @types.U64Type(_) => @types.U64Type(r)
      @types.F32Type(_) => @types.F32Type(r)
      @types.F64Type(_) => @types.F64Type(r)
      _ => abort("numeric type checked above")
    },
  )
}

///|
pub fn wire_tool_literal(
  type_ : WireToolType,
  literal : ToolLiteral,
) -> @types.SchemaValueTree raise ToolBuildError {
  let encoder = @schema.WireEncoder::new()
  let root = append_tool_literal(type_, literal, encoder, 128)
  { value_nodes: encoder.nodes, root, }
}

///|
pub fn wire_value_is_literal(
  type_ : WireToolType,
  literal : ToolLiteral,
) -> @types.SchemaValueTree raise ToolBuildError {
  wire_tool_literal(type_, literal) catch {
    original => {
      let item = for current = type_, remaining = type_.encoder.nodes.length(); remaining >
                    0; {
        match current.resolved().body {
          @types.OptionType(inner) =>
            continue { encoder: type_.encoder, index: inner, }, remaining - 1
          @types.ListType(inner) => break inner
          @types.FixedListType(spec) => break spec.element
          @types.MapType(spec) => break spec.value
          _ => raise original
        }
      } nobreak {
        raise original
      }
      wire_tool_literal({ encoder: type_.encoder, index: item, }, literal) catch {
        _ => raise original
      }
    }
  }
}

///|
fn append_tool_literal(
  type_ : WireToolType,
  literal : ToolLiteral,
  encoder : @schema.WireEncoder,
  remaining : Int,
) -> Int raise ToolBuildError {
  guard remaining > 0 else {
    raise InvalidLiteral("recursive metadata literal")
  }
  let child = (index, literal) => {
    append_tool_literal(
      { encoder: type_.encoder, index, },
      literal,
      encoder,
      remaining - 1,
    )
  }
  let node = match (type_.resolved().body, literal) {
    (@types.OptionType(inner), literal) =>
      @types.OptionValue(Some(child(inner, literal)))
    (@types.ListType(inner), List(tokens)) =>
      @types.ListValue(tokens.map(token => child(inner, Scalar(token))))
    (@types.FixedListType(spec), List(tokens)) => {
      guard tokens.length().reinterpret_as_uint() == spec.length else {
        raise InvalidLiteral("fixed-list metadata length mismatch")
      }
      @types.FixedListValue(
        tokens.map(token => child(spec.element, Scalar(token))),
      )
    }
    (@types.MapType(spec), Map(entries)) =>
      @types.MapValue(
        entries.map(entry => {
          key: child(spec.key, Scalar(entry.0)),
          value: child(spec.value, Scalar(entry.1)),
        }),
      )
    (@types.MapType(_), List([])) => @types.MapValue([])
    (@types.BoolType, Scalar(token)) =>
      @types.BoolValue(parse_authored(token, "bool"))
    (@types.S8Type(_), Scalar(token)) => {
      let value : Int = parse_authored(token, "s8")
      guard value >= -128 && value <= 127 else {
        raise InvalidLiteral("s8 out of range")
      }
      @types.S8Value(value)
    }
    (@types.S16Type(_), Scalar(token)) => {
      let value : Int = parse_authored(token, "s16")
      guard value >= -32768 && value <= 32767 else {
        raise InvalidLiteral("s16 out of range")
      }
      @types.S16Value(value)
    }
    (@types.S32Type(_), Scalar(token)) =>
      @types.S32Value(parse_authored(token, "s32"))
    (@types.S64Type(_), Scalar(token)) =>
      @types.S64Value(parse_authored(token, "s64"))
    (@types.U8Type(_), Scalar(token)) => {
      let value : UInt = parse_authored(token, "u8")
      guard value <= 255 else { raise InvalidLiteral("u8 out of range") }
      @types.U8Value(value.to_byte())
    }
    (@types.U16Type(_), Scalar(token)) => {
      let value : UInt = parse_authored(token, "u16")
      guard value <= 65535 else { raise InvalidLiteral("u16 out of range") }
      @types.U16Value(value)
    }
    (@types.U32Type(_), Scalar(token)) =>
      @types.U32Value(parse_authored(token, "u32"))
    (@types.U64Type(_), Scalar(token)) =>
      @types.U64Value(parse_authored(token, "u64"))
    (@types.F32Type(_), Scalar(token)) =>
      @types.F32Value(Float::from_double(parse_authored(token, "f32")))
    (@types.F64Type(_), Scalar(token)) =>
      @types.F64Value(parse_authored(token, "f64"))
    (@types.CharType, Scalar(token)) => @types.CharValue(parse_char(token))
    (@types.StringType, Scalar(token)) => @types.StringValue(token)
    (@types.TextType(_), Scalar(token)) =>
      @types.TextValue({ text: token, language: None, })
    (@types.PathType(_), Scalar(token)) => @types.PathValue(token)
    (@types.UrlType(_), Scalar(token)) => @types.UrlValue(token)
    (@types.EnumType(cases), Scalar(token)) => {
      guard cases.search_by(case => case == token) is Some(index) else {
        raise InvalidLiteral("unknown enum case '\{token}'")
      }
      @types.EnumValue(index.reinterpret_as_uint())
    }
    _ => raise LiteralTypeMismatch("literal does not match generated wire type")
  }
  encoder.push(node)
}
