///|
/// Canonical HTTP field. Values are bytes, and repeated fields remain separate.
pub(all) struct HttpHeader {
  name : String
  value : Array[Byte]
} derive(Eq)

///|
/// The original public request target and a demand-driven, transfer-decoded body.
/// An empty body chunk is data, not EOF. Body contents must not be logged.
pub(all) struct HttpRequest {
  method : String
  scheme : String
  authority : String
  path : String
  query : String?
  headers : Array[HttpHeader]
  body : @schema.AgentStream[Array[Byte]]
}

///|
/// A streaming response. The host validates status and framing before commitment.
/// HEAD and bodyless statuses dispose the producer without polling its items.
pub(all) struct HttpResponse {
  status : UInt16
  headers : Array[HttpHeader]
  body : @schema.AgentStream[Array[Byte]]
}

///|
pub impl @schema.IntoSchema for HttpHeader with fn type_id() {
  "golem.http.HttpHeader"
}

///|
pub impl @schema.IntoSchema for HttpHeader with fn register_in(_b) {
  @model.schema_type(
    Record([
      @model.field("name", @model.schema_type(String)),
      @model.field(
        "value",
        @model.schema_type(List(@model.schema_type(U8(None)))),
      ),
    ]),
  )
}

///|
pub impl @schema.IntoSchema for HttpHeader with fn to_value(self) {
  @model.SchemaValue::Record([
    @schema.to_value_as(self.name),
    @schema.to_value_as(self.value),
  ])
}

///|
pub impl @schema.FromSchema for HttpHeader with fn from_value(value) {
  match value {
    @model.Record([name, value]) =>
      {
        name: @schema.from_value_as(name),
        value: @schema.from_value_as(value),
      }
    _ => raise @schema.FromSchemaError::custom("invalid HTTP header record")
  }
}

///|
pub impl @schema.IntoSchema for HttpRequest with fn type_id() {
  "golem.http.HttpRequest"
}

///|
pub impl @schema.IntoSchema for HttpRequest with fn register_in(b) {
  @model.schema_type(
    Record([
      @model.field("method", @model.schema_type(String)),
      @model.field("scheme", @model.schema_type(String)),
      @model.field("authority", @model.schema_type(String)),
      @model.field("path", @model.schema_type(String)),
      @model.field(
        "query",
        @schema.register_in_with(
          b,
          (@schema.type_tag() : @schema.TypeTag[String?]),
        ),
      ),
      @model.field(
        "headers",
        @schema.register_in_with(
          b,
          (@schema.type_tag() : @schema.TypeTag[Array[HttpHeader]]),
        ),
      ),
      @model.field(
        "body",
        @schema.register_in_with(
          b,
          (
            @schema.type_tag() :
            @schema.TypeTag[@schema.AgentStream[Array[Byte]]]),
        ),
      ),
    ]),
  )
}

///|
pub impl @schema.IntoSchema for HttpRequest with fn to_value(self) {
  @model.SchemaValue::Record([
    @schema.to_value_as(self.method),
    @schema.to_value_as(self.scheme),
    @schema.to_value_as(self.authority),
    @schema.to_value_as(self.path),
    @schema.to_value_as(self.query),
    @schema.to_value_as(self.headers),
    @schema.to_value_as(self.body),
  ])
}

///|
pub impl @schema.FromSchema for HttpRequest with fn from_value(value) {
  match value {
    @model.Record([verb, scheme, authority, path, query, headers, body]) =>
      {
        method: @schema.from_value_as(verb),
        scheme: @schema.from_value_as(scheme),
        authority: @schema.from_value_as(authority),
        path: @schema.from_value_as(path),
        query: @schema.from_value_as(query),
        headers: @schema.from_value_as(headers),
        body: @schema.from_value_as(body),
      }
    _ => raise @schema.FromSchemaError::custom("invalid HTTP request record")
  }
}

///|
pub impl @schema.IntoSchema for HttpResponse with fn type_id() {
  "golem.http.HttpResponse"
}

///|
pub impl @schema.IntoSchema for HttpResponse with fn register_in(b) {
  @model.schema_type(
    Record([
      @model.field("status", @model.schema_type(U16(None))),
      @model.field(
        "headers",
        @schema.register_in_with(
          b,
          (@schema.type_tag() : @schema.TypeTag[Array[HttpHeader]]),
        ),
      ),
      @model.field(
        "body",
        @schema.register_in_with(
          b,
          (
            @schema.type_tag() :
            @schema.TypeTag[@schema.AgentStream[Array[Byte]]]),
        ),
      ),
    ]),
  )
}

///|
pub impl @schema.IntoSchema for HttpResponse with fn to_value(self) {
  @model.SchemaValue::Record([
    @schema.to_value_as(self.status),
    @schema.to_value_as(self.headers),
    @schema.to_value_as(self.body),
  ])
}

///|
pub impl @schema.FromSchema for HttpResponse with fn from_value(value) {
  match value {
    @model.Record([status, headers, body]) =>
      {
        status: @schema.from_value_as(status),
        headers: @schema.from_value_as(headers),
        body: @schema.from_value_as(body),
      }
    _ => raise @schema.FromSchemaError::custom("invalid HTTP response record")
  }
}
