// User-facing unstructured text and binary values.
//
// An unstructured value is either inline content or a URL reference. Its
// schema is the canonical role-marked wrapper shared by every Golem SDK and the
// host:
//
//   variant { inline: text<languages>, url: url }    role = unstructured-text
//   variant { inline: binary<mime-types>, url: url } role = unstructured-binary
//
// The `inline` case has index 0 and the `url` case has index 1. The wrapper is
// always emitted inline (never as a named `Ref` definition) because the host
// detects it from the node's own role marker.
//
// Restrictions are type-level, like `Text[S]` and `Quantity[U]`: the phantom
// type argument implements `AllowedLanguages` or `AllowedMimeTypes`, and its
// static method lists the allowed codes. `AnyLanguage` and `AnyMimeType`
// allow every value. Decoders accept the wrapper variant or a bare text/binary
// value, and reject an inline language or MIME type that the restriction set
// does not allow. Constructors do not validate.

///|
/// The set of language codes allowed for an `UnstructuredText[L]`.
/// An empty array allows every language.
pub(open) trait AllowedLanguages {
  fn languages() -> Array[String]
}

///|
/// The set of MIME types allowed for an `UnstructuredBinary[M]`.
/// An empty array allows every MIME type.
pub(open) trait AllowedMimeTypes {
  fn mime_types() -> Array[String]
}

///|
/// Language restriction that allows every language code.
#warnings("-struct_never_constructed")
pub(all) struct AnyLanguage {} derive(Debug, Eq)

///|
pub impl AllowedLanguages for AnyLanguage with fn languages() {
  []
}

///|
/// MIME type restriction that allows every MIME type.
#warnings("-struct_never_constructed")
pub(all) struct AnyMimeType {} derive(Debug, Eq)

///|
pub impl AllowedMimeTypes for AnyMimeType with fn mime_types() {
  []
}

///|
/// Unstructured text: inline text with an optional language code, or a URL
/// that references the text. `L` lists the allowed language codes.
#warnings("-unused_type_variable")
pub(all) enum UnstructuredText[L] {
  Inline(text~ : String, language~ : String?)
  Url(String)
}

///|
/// Equality ignores the phantom restriction type, which carries no data.
pub impl[L] Eq for UnstructuredText[L] with fn equal(self, other) {
  match (self, other) {
    (Inline(text~, language~), Inline(text=other_text, language=other_language)) =>
      text == other_text && language == other_language
    (Url(url), Url(other_url)) => url == other_url
    _ => false
  }
}

///|
pub impl[L] Debug for UnstructuredText[L] with fn to_repr(self) {
  match self {
    Inline(text~, language~) =>
      Repr::ctor("Inline", [
        (Some("text"), Repr::string(text)),
        (Some("language"), Repr(language)),
      ])
    Url(url) => Repr::ctor("Url", [(None, Repr::string(url))])
  }
}

///|
/// Create inline unstructured text with an optional language code.
pub fn[L] UnstructuredText::inline(
  text : String,
  language? : String,
) -> UnstructuredText[L] {
  Inline(text~, language~)
}

///|
/// Create unstructured text that references its content by URL.
pub fn[L] UnstructuredText::url(url : String) -> UnstructuredText[L] {
  Url(url)
}

///|
/// Unstructured binary: inline bytes with an optional MIME type, or a URL that
/// references the bytes. `M` lists the allowed MIME types.
#warnings("-unused_type_variable")
pub(all) enum UnstructuredBinary[M] {
  Inline(bytes~ : Bytes, mime_type~ : String?)
  Url(String)
}

///|
/// Equality ignores the phantom restriction type, which carries no data.
pub impl[M] Eq for UnstructuredBinary[M] with fn equal(self, other) {
  match (self, other) {
    (
      Inline(bytes~, mime_type~),
      Inline(bytes=other_bytes, mime_type=other_mime_type),
    ) => bytes == other_bytes && mime_type == other_mime_type
    (Url(url), Url(other_url)) => url == other_url
    _ => false
  }
}

///|
pub impl[M] Debug for UnstructuredBinary[M] with fn to_repr(self) {
  match self {
    Inline(bytes~, mime_type~) =>
      Repr::ctor("Inline", [
        (Some("bytes"), Repr(bytes)),
        (Some("mime_type"), Repr(mime_type)),
      ])
    Url(url) => Repr::ctor("Url", [(None, Repr::string(url))])
  }
}

///|
/// Create inline unstructured binary with an optional MIME type.
pub fn[M] UnstructuredBinary::inline(
  bytes : Bytes,
  mime_type? : String,
) -> UnstructuredBinary[M] {
  Inline(bytes~, mime_type~)
}

///|
/// Create unstructured binary that references its content by URL.
pub fn[M] UnstructuredBinary::url(url : String) -> UnstructuredBinary[M] {
  Url(url)
}

///|
/// Case index of the `inline` alternative of an unstructured wrapper.
let unstructured_inline_case : UInt = 0

///|
/// Case index of the `url` alternative of an unstructured wrapper.
let unstructured_url_case : UInt = 1

///|
fn role_metadata(role : @types.Role) -> @types.MetadataEnvelope {
  { ..empty_wire_metadata(), role: Some(role), }
}

///|
fn non_empty(values : Array[String]) -> Array[String]? {
  if values.is_empty() {
    None
  } else {
    Some(values)
  }
}

///|
fn[L : AllowedLanguages] unstructured_text_restrictions(
  _tag : TypeTag[L],
) -> @types.TextRestrictions {
  {
    languages: non_empty(L::languages()),
    min_length: None,
    max_length: None,
    regex: None,
  }
}

///|
fn[M : AllowedMimeTypes] unstructured_binary_restrictions(
  _tag : TypeTag[M],
) -> @types.BinaryRestrictions {
  { mime_types: non_empty(M::mime_types()), min_bytes: None, max_bytes: None, }
}

///|
fn check_allowed(
  value : String?,
  allowed : Array[String],
  kind : String,
) -> String? {
  match value {
    Some(value) if !allowed.is_empty() && !allowed.contains(value) =>
      Some("\{kind} '\{value}' is not allowed; allowed: \{allowed.join(", ")}")
    _ => None
  }
}

///|
fn[L : AllowedLanguages] check_language(
  _tag : TypeTag[L],
  language : String?,
) -> String? {
  check_allowed(language, L::languages(), "language code")
}

///|
fn[M : AllowedMimeTypes] check_mime_type(
  _tag : TypeTag[M],
  mime_type : String?,
) -> String? {
  check_allowed(mime_type, M::mime_types(), "MIME type")
}

///|
fn[L : AllowedLanguages] text_from_parts(
  text : String,
  language : String?,
) -> UnstructuredText[L] raise FromSchemaError {
  match check_language((type_tag() : TypeTag[L]), language) {
    Some(message) => raise FromSchemaError::custom(message)
    None => Inline(text~, language~)
  }
}

///|
fn[M : AllowedMimeTypes] binary_from_parts(
  bytes : FixedArray[Byte],
  mime_type : String?,
) -> UnstructuredBinary[M] raise FromSchemaError {
  match check_mime_type((type_tag() : TypeTag[M]), mime_type) {
    Some(message) => raise FromSchemaError::custom(message)
    None => Inline(bytes=Bytes::from_array(bytes[:]), mime_type~)
  }
}

// ============================================================
// Recursive schema model
// ============================================================

///|
pub impl[L : AllowedLanguages] IntoSchema for UnstructuredText[L] with fn type_id() {
  type_id_with_args("golem.core.UnstructuredText", L::languages())
}

///|
pub impl[L : AllowedLanguages] IntoSchema for UnstructuredText[L] with fn register_in(
  _b,
) {
  let restrictions = unstructured_text_restrictions((type_tag() : TypeTag[L]))
  @model.schema_type(
    Variant([
      @model.variant_case(
        "inline",
        payload=@model.schema_type(Text(restrictions)),
      ),
      @model.variant_case(
        "url",
        payload=@model.schema_type(Url(default_url_restrictions())),
      ),
    ]),
    metadata=role_metadata(@types.Role::UnstructuredText),
  )
}

///|
pub impl[L : AllowedLanguages] IntoSchema for UnstructuredText[L] with fn to_value(
  self,
) {
  match self {
    Inline(text~, language~) =>
      Variant(unstructured_inline_case, Some(Text(text, language)))
    Url(url) => Variant(unstructured_url_case, Some(Url(url)))
  }
}

///|
pub impl[L : AllowedLanguages] FromSchema for UnstructuredText[L] with fn from_value(
  v,
) {
  match v {
    Variant(0, Some(Text(text, language))) | Text(text, language) =>
      text_from_parts(text, language)
    Variant(1, Some(Url(url))) => Url(url)
    Variant(case, _) if case > 1 =>
      raise FromSchemaError::out_of_range(case, 2, "UnstructuredText")
    Variant(_, Some(payload)) =>
      raise FromSchemaError::shape_mismatch(
        "text or url",
        value_kind(payload),
        "UnstructuredText",
      )
    Variant(_, None) =>
      raise FromSchemaError::custom(
        "UnstructuredText case is missing its payload",
      )
    other =>
      raise FromSchemaError::shape_mismatch(
        "variant or text",
        value_kind(other),
        "UnstructuredText",
      )
  }
}

///|
pub impl[M : AllowedMimeTypes] IntoSchema for UnstructuredBinary[M] with fn type_id() {
  type_id_with_args("golem.core.UnstructuredBinary", M::mime_types())
}

///|
pub impl[M : AllowedMimeTypes] IntoSchema for UnstructuredBinary[M] with fn register_in(
  _b,
) {
  let restrictions = unstructured_binary_restrictions((type_tag() : TypeTag[M]))
  @model.schema_type(
    Variant([
      @model.variant_case(
        "inline",
        payload=@model.schema_type(Binary(restrictions)),
      ),
      @model.variant_case(
        "url",
        payload=@model.schema_type(Url(default_url_restrictions())),
      ),
    ]),
    metadata=role_metadata(@types.Role::UnstructuredBinary),
  )
}

///|
pub impl[M : AllowedMimeTypes] IntoSchema for UnstructuredBinary[M] with fn to_value(
  self,
) {
  match self {
    Inline(bytes~, mime_type~) =>
      Variant(
        unstructured_inline_case,
        Some(Binary(bytes.to_fixedarray(), mime_type)),
      )
    Url(url) => Variant(unstructured_url_case, Some(Url(url)))
  }
}

///|
pub impl[M : AllowedMimeTypes] FromSchema for UnstructuredBinary[M] with fn from_value(
  v,
) {
  match v {
    Variant(0, Some(Binary(bytes, mime_type))) | Binary(bytes, mime_type) =>
      binary_from_parts(bytes, mime_type)
    Variant(1, Some(Url(url))) => Url(url)
    Variant(case, _) if case > 1 =>
      raise FromSchemaError::out_of_range(case, 2, "UnstructuredBinary")
    Variant(_, Some(payload)) =>
      raise FromSchemaError::shape_mismatch(
        "binary or url",
        value_kind(payload),
        "UnstructuredBinary",
      )
    Variant(_, None) =>
      raise FromSchemaError::custom(
        "UnstructuredBinary case is missing its payload",
      )
    other =>
      raise FromSchemaError::shape_mismatch(
        "variant or binary",
        value_kind(other),
        "UnstructuredBinary",
      )
  }
}

// ============================================================
// Flat wire carrier
// ============================================================

///|
fn append_unstructured_wire_type(
  encoder : WireTypeEncoder,
  inline : @types.SchemaTypeBody,
  role : @types.Role,
) -> Int {
  let inline = encoder.push(inline)
  let url = encoder.push(@types.UrlType(default_url_restrictions()))
  encoder.push_with_metadata(
    @types.VariantType([
      {
        name: "inline",
        payload: Some(inline),
        metadata: empty_wire_metadata(),
      },
      { name: "url", payload: Some(url), metadata: empty_wire_metadata(), },
    ]),
    role_metadata(role),
  )
}

///|
fn append_unstructured_wire_value(
  encoder : WireEncoder,
  case : UInt,
  payload : @types.SchemaValueNode,
) -> Int {
  let payload = encoder.push(payload)
  encoder.push(@types.VariantValue({ case, payload: Some(payload), }))
}

///|
pub impl[L : AllowedLanguages] WireSchema for UnstructuredText[L] with fn append_wire_type(
  encoder,
) {
  append_unstructured_wire_type(
    encoder,
    @types.TextType(unstructured_text_restrictions((type_tag() : TypeTag[L]))),
    @types.Role::UnstructuredText,
  )
}

///|
pub impl[L] IntoWire for UnstructuredText[L] with fn append_wire(self, encoder) {
  match self {
    Inline(text~, language~) =>
      append_unstructured_wire_value(
        encoder,
        unstructured_inline_case,
        @types.TextValue({
          text: wire_text(text, "unstructured text"),
          language: wire_optional_text(language, "unstructured text language"),
        }),
      )
    Url(url) =>
      append_unstructured_wire_value(
        encoder,
        unstructured_url_case,
        @types.UrlValue(wire_text(url, "unstructured url")),
      )
  }
}

///|
fn[L : AllowedLanguages] read_text_wire(
  text : String,
  language : String?,
) -> UnstructuredText[L] raise WireError {
  match check_language((type_tag() : TypeTag[L]), language) {
    Some(message) => raise WireError::Invalid("UnstructuredText: \{message}")
    None => Inline(text~, language~)
  }
}

///|
pub impl[L : AllowedLanguages] FromWire for UnstructuredText[L] with fn read_wire(
  reader,
  index,
) {
  match wire_node(reader, index) {
    @types.TextValue(value) => read_text_wire(value.text, value.language)
    @types.VariantValue({ case, payload, }) =>
      match (case, wire_node(reader, wire_payload(payload))) {
        (0, @types.TextValue(value)) =>
          read_text_wire(value.text, value.language)
        (1, @types.UrlValue(url)) => Url(url)
        _ =>
          raise WireError::Invalid(
            "UnstructuredText: expected inline text or url payload",
          )
      }
    _ =>
      raise WireError::Invalid(
        "UnstructuredText: expected variant or text value",
      )
  }
}

///|
pub impl[M : AllowedMimeTypes] WireSchema for UnstructuredBinary[M] with fn append_wire_type(
  encoder,
) {
  append_unstructured_wire_type(
    encoder,
    @types.BinaryType(
      unstructured_binary_restrictions((type_tag() : TypeTag[M])),
    ),
    @types.Role::UnstructuredBinary,
  )
}

///|
pub impl[M] IntoWire for UnstructuredBinary[M] with fn append_wire(
  self,
  encoder,
) {
  match self {
    Inline(bytes~, mime_type~) =>
      append_unstructured_wire_value(
        encoder,
        unstructured_inline_case,
        @types.BinaryValue({
          bytes: bytes.to_fixedarray(),
          mime_type: wire_optional_text(
            mime_type, "unstructured binary mime type",
          ),
        }),
      )
    Url(url) =>
      append_unstructured_wire_value(
        encoder,
        unstructured_url_case,
        @types.UrlValue(wire_text(url, "unstructured url")),
      )
  }
}

///|
fn[M : AllowedMimeTypes] read_binary_wire(
  bytes : FixedArray[Byte],
  mime_type : String?,
) -> UnstructuredBinary[M] raise WireError {
  match check_mime_type((type_tag() : TypeTag[M]), mime_type) {
    Some(message) => raise WireError::Invalid("UnstructuredBinary: \{message}")
    None => Inline(bytes=Bytes::from_array(bytes[:]), mime_type~)
  }
}

///|
pub impl[M : AllowedMimeTypes] FromWire for UnstructuredBinary[M] with fn read_wire(
  reader,
  index,
) {
  match wire_node(reader, index) {
    @types.BinaryValue(value) => read_binary_wire(value.bytes, value.mime_type)
    @types.VariantValue({ case, payload, }) =>
      match (case, wire_node(reader, wire_payload(payload))) {
        (0, @types.BinaryValue(value)) =>
          read_binary_wire(value.bytes, value.mime_type)
        (1, @types.UrlValue(url)) => Url(url)
        _ =>
          raise WireError::Invalid(
            "UnstructuredBinary: expected inline binary or url payload",
          )
      }
    _ =>
      raise WireError::Invalid(
        "UnstructuredBinary: expected variant or binary value",
      )
  }
}
