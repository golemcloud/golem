///|
fn schema_kind_name(body : @model.SchemaTypeBody) -> String {
  match body {
    Ref(_) => "ref"
    Bool => "bool"
    S8(_)
    | S16(_)
    | S32(_)
    | S64(_)
    | U8(_)
    | U16(_)
    | U32(_)
    | U64(_)
    | F32(_)
    | F64(_) => "numeric"
    Char => "char"
    String => "string"
    Text(_) => "text"
    Path(_) => "path"
    Url(_) => "url"
    Record(_) => "record"
    Variant(_) => "variant"
    Enum(_) => "enum"
    Flags(_) => "flags"
    Tuple(_) => "tuple"
    List(_) => "list"
    FixedList(_, _) => "fixed-list"
    Map(_, _) => "map"
    Option(_) => "option"
    Result(_, _) => "result"
    Binary(_) => "binary"
    Datetime => "datetime"
    Duration => "duration"
    Quantity(_) => "quantity"
    Union(_) => "union"
    Secret(_) => "secret"
    QuotaToken(_) => "quota-token"
    Future(_) => "future"
    Stream(_) => "stream"
  }
}

///|
fn[T] overlay(authored : T?, existing : T?) -> T? {
  match authored {
    Some(value) => Some(value)
    None => existing
  }
}

///|
fn refinement_root(
  graph : @model.SchemaGraph,
  refinement : String,
) -> @model.SchemaType raise ToolBuildError {
  resolve_literal_type(graph, graph.root) catch {
    _ => raise RefinementTypeMismatch(refinement, "unresolved-ref")
  }
}

///|
pub fn refine_text(
  graph : @model.SchemaGraph,
  regex : String?,
  min_length : UInt?,
  max_length : UInt?,
) -> @model.SchemaGraph raise ToolBuildError {
  let root = refinement_root(graph, "text")
  let current : @types.TextRestrictions = match root.body {
    @model.SchemaTypeBody::String =>
      { languages: None, min_length: None, max_length: None, regex: None }
    @model.SchemaTypeBody::Text(restrictions) => restrictions
    other => raise RefinementTypeMismatch("text", schema_kind_name(other))
  }
  if min_length is Some(min) && max_length is Some(max) && min > max {
    raise InvalidLiteral(
      "text minimum length \{min} exceeds maximum length \{max}",
    )
  }
  {
    defs: graph.defs,
    root: {
      body: @model.SchemaTypeBody::Text({
        languages: current.languages,
        min_length: overlay(min_length, current.min_length),
        max_length: overlay(max_length, current.max_length),
        regex: overlay(regex, current.regex),
      }),
      metadata: root.metadata,
    },
  }
}

///|
pub fn refine_path(
  graph : @model.SchemaGraph,
  direction : @types.PathDirection?,
  kind : @types.PathKind?,
  mime_types : Array[String]?,
) -> @model.SchemaGraph raise ToolBuildError {
  let root = refinement_root(graph, "path")
  let spec = match root.body {
    @model.SchemaTypeBody::Path(spec) => spec
    other => raise RefinementTypeMismatch("path", schema_kind_name(other))
  }
  {
    defs: graph.defs,
    root: {
      body: @model.SchemaTypeBody::Path({
        direction: direction.unwrap_or(spec.direction),
        kind: kind.unwrap_or(spec.kind),
        allowed_mime_types: overlay(mime_types, spec.allowed_mime_types),
        allowed_extensions: spec.allowed_extensions,
      }),
      metadata: root.metadata,
    },
  }
}

///|
pub fn refine_url(
  graph : @model.SchemaGraph,
  schemes : Array[String]?,
) -> @model.SchemaGraph raise ToolBuildError {
  let root = refinement_root(graph, "url")
  let restrictions = match root.body {
    @model.SchemaTypeBody::Url(restrictions) => restrictions
    other => raise RefinementTypeMismatch("url", schema_kind_name(other))
  }
  {
    defs: graph.defs,
    root: {
      body: @model.SchemaTypeBody::Url({
        allowed_schemes: overlay(schemes, restrictions.allowed_schemes),
        allowed_hosts: restrictions.allowed_hosts,
      }),
      metadata: root.metadata,
    },
  }
}

///|
fn numeric_bound(
  token : String,
  body : @model.SchemaTypeBody,
) -> @types.NumericBound raise ToolBuildError {
  match body {
    S8(_) | S16(_) | S32(_) | S64(_) => {
      let value : Int64 = @strconv.from_str(token) catch {
        _ =>
          raise InvalidNumericBound("invalid signed numeric bound '\{token}'")
      }
      let in_range = match body {
        S8(_) => value >= -128L && value <= 127L
        S16(_) => value >= -32768L && value <= 32767L
        S32(_) => value >= -2147483648L && value <= 2147483647L
        _ => true
      }
      guard in_range else {
        raise InvalidNumericBound("numeric bound '\{token}' is out of range")
      }
      @types.NumericBound::Signed(value)
    }
    U8(_) | U16(_) | U32(_) | U64(_) => {
      let value : UInt64 = @strconv.from_str(token) catch {
        _ =>
          raise InvalidNumericBound("invalid unsigned numeric bound '\{token}'")
      }
      let in_range = match body {
        U8(_) => value <= 255UL
        U16(_) => value <= 65535UL
        U32(_) => value <= 4294967295UL
        _ => true
      }
      guard in_range else {
        raise InvalidNumericBound("numeric bound '\{token}' is out of range")
      }
      @types.NumericBound::Unsigned(value)
    }
    F32(_) | F64(_) => {
      let value : Double = @strconv.from_str(token) catch {
        _ =>
          raise InvalidNumericBound("invalid floating-point bound '\{token}'")
      }
      guard !value.is_nan() && !value.is_inf() else {
        raise InvalidNumericBound(
          "floating-point bound '\{token}' must be finite",
        )
      }
      let bits = if value == 0.0 { 0UL } else { value.reinterpret_as_uint64() }
      @types.NumericBound::FloatBits(bits)
    }
    other => raise RefinementTypeMismatch("numeric", schema_kind_name(other))
  }
}

///|
fn bound_greater(
  left : @types.NumericBound,
  right : @types.NumericBound,
) -> Bool {
  match (left, right) {
    (@types.Signed(a), @types.Signed(b)) => a > b
    (@types.Unsigned(a), @types.Unsigned(b)) => a > b
    (@types.FloatBits(a), @types.FloatBits(b)) =>
      a.reinterpret_as_double() > b.reinterpret_as_double()
    _ => false
  }
}

///|
pub fn refine_numeric(
  graph : @model.SchemaGraph,
  min : String?,
  max : String?,
  unit : String?,
) -> @model.SchemaGraph raise ToolBuildError {
  let root = refinement_root(graph, "numeric")
  let existing = match root.body {
    S8(r)
    | S16(r)
    | S32(r)
    | S64(r)
    | U8(r)
    | U16(r)
    | U32(r)
    | U64(r)
    | F32(r)
    | F64(r) => r
    other => raise RefinementTypeMismatch("numeric", schema_kind_name(other))
  }
  let current : @types.NumericRestrictions = existing.unwrap_or({
    min: None,
    max: None,
    unit: None,
  })
  let min_bound = min.map(token => numeric_bound(token, root.body))
  let max_bound = max.map(token => numeric_bound(token, root.body))
  let refined : @types.NumericRestrictions = {
    min: overlay(min_bound, current.min),
    max: overlay(max_bound, current.max),
    unit: overlay(unit, current.unit),
  }
  if refined.min is Some(lo) && refined.max is Some(hi) && bound_greater(lo, hi) {
    raise InvalidNumericBound("numeric minimum exceeds maximum")
  }
  let restrictions = if refined.min is None &&
    refined.max is None &&
    refined.unit is None {
    None
  } else {
    Some(refined)
  }
  let body = match root.body {
    S8(_) => @model.SchemaTypeBody::S8(restrictions)
    S16(_) => @model.SchemaTypeBody::S16(restrictions)
    S32(_) => @model.SchemaTypeBody::S32(restrictions)
    S64(_) => @model.SchemaTypeBody::S64(restrictions)
    U8(_) => @model.SchemaTypeBody::U8(restrictions)
    U16(_) => @model.SchemaTypeBody::U16(restrictions)
    U32(_) => @model.SchemaTypeBody::U32(restrictions)
    U64(_) => @model.SchemaTypeBody::U64(restrictions)
    F32(_) => @model.SchemaTypeBody::F32(restrictions)
    F64(_) => @model.SchemaTypeBody::F64(restrictions)
    _ => abort("numeric kind checked above")
  }
  { defs: graph.defs, root: { body, metadata: root.metadata } }
}
