///|
#derive.tool("moonbit-reflection-test")
struct MoonBitReflectionTest {}

///|
#warnings("-struct_never_constructed")
struct ReflectionMeters {}

///|
impl @schema.QuantityUnit for ReflectionMeters with fn type_id() {
  "golem.examples.ReflectionMeters"
}

///|
impl @schema.QuantityUnit for ReflectionMeters with fn base_unit() {
  "m"
}

///|
impl @schema.QuantityUnit for ReflectionMeters with fn allowed_suffixes() {
  []
}

///|
#derive.golem_schema
pub(all) struct AuditLogConstructor {
  name : String
}

///|
pub(all) struct EmptyAgentInput {}

///|
pub impl @schema.IntoSchema for EmptyAgentInput with fn type_id() {
  "golem_moonbit_examples.EmptyAgentInput"
}

///|
pub impl @schema.IntoSchema for EmptyAgentInput with fn register_in(_b) {
  @model.schema_type(Record([]))
}

///|
pub impl @schema.IntoSchema for EmptyAgentInput with fn to_value(_self) {
  Record([])
}

///|
pub fn MoonBitReflectionTest::echo(label : String) -> String {
  "moonbit-tool:" + label
}

///|
#derive.arg("maybe", scope="option")
pub fn MoonBitReflectionTest::optional(maybe : String?) -> String {
  maybe.unwrap_or("omitted")
}

///|
#derive.arg("maybe", scope="option")
pub fn MoonBitReflectionTest::canonical_values(
  signed : Int64,
  unsigned : UInt64,
  duration : @schema.Duration,
  quantity : @schema.Quantity[ReflectionMeters],
  maybe : String?,
) -> String {
  let quantity = quantity.value()
  if signed == -9223372036854775807L - 1L &&
    unsigned == 18446744073709551615UL &&
    duration == @schema.Duration::new(9223372036854775807L) &&
    quantity.mantissa == -9223372036854775807L - 1L &&
    quantity.scale == -9 &&
    quantity.unit == "m" &&
    maybe is None {
    "moonbit-canonical-ok"
  } else {
    "moonbit-canonical-mismatch"
  }
}

///|
fn reflected_tool_error(
  error : @tool.ToolError[@reflection.ReflectedToolCustomError],
) -> String {
  match error {
    Rpc(error) => "rpc:\{Repr(error)}"
    RemoteTool(error) => "remote:\{Repr(error)}"
    Tool(error) => {
      let message = "tool:\{Repr(error)}"
      @model.drop_owned_capabilities(error.payload.value)
      message
    }
    UnknownToolError(name, payload) => {
      @model.drop_owned_capabilities(payload.value)
      "unknown:\{name}"
    }
    InvalidInput(message) => "input:\{message}"
    MalformedRemoteOutput(message) => "output:\{message}"
  }
}

///|
fn describe_dynamic_tool_error(
  error : @tool.ToolError[@reflection.ReflectedToolCustomError],
) -> String {
  match error {
    Rpc(error) => "rpc:\{Repr(error)}"
    RemoteTool(error) => "remote:\{Repr(error)}"
    Tool(custom) => {
      let message = "tool:\{Repr(custom)}"
      @model.drop_owned_capabilities(custom.payload.value)
      message
    }
    UnknownToolError(name, payload) => {
      @model.drop_owned_capabilities(payload.value)
      "unknown:\{name}"
    }
    InvalidInput(message) => "input:\{message}"
    MalformedRemoteOutput(message) => "output:\{message}"
  }
}

///|
async fn invoke_discovered_tool_dynamically() -> String {
  let tool = @reflection.get_tool_type("moonbit-reflection-test")
  let command = tool.command(["echo"])
  let packed = match
    command.pack_json(Json::object({ "label": Json::string("moonbit") })) {
    Ok(value) => value
    Err(error) => return "input:\{describe_dynamic_tool_error(error)}"
  }
  command.input_schema().validate_value(packed) catch {
    error => return "input:\{Repr(error)}"
  }
  let typed_input = command.input_schema().typed_value(packed) catch {
      error => return "input:\{Repr(error)}"
    }

  let dynamic = @reflection.DynamicToolClient::new(tool.lookup_name())
  let raw = match dynamic.invoke_value(command.path(), typed_input) {
    Ok(raw) => raw
    Err(error) => return "invoke:\{describe_dynamic_tool_error(error)}"
  }

  match (command.result(), raw.result) {
    (None, None) => "ok"
    (Some(schema), Some(output)) => {
      schema.validate_value(output.value) catch {
        error => {
          @model.drop_owned_capabilities(output.value)
          return "output:\{Repr(error)}"
        }
      }
      let json = schema.unpack_json(output.value) catch {
        error => {
          @model.drop_owned_capabilities(output.value)
          return "decode:\{Repr(error)}"
        }
      }
      @model.drop_owned_capabilities(output.value)
      json.stringify()
    }
    (_, Some(output)) => {
      @model.drop_owned_capabilities(output.value)
      "output:unexpected"
    }
    _ => "output:missing"
  }
}

///|
fn no_tool_error(error : @tool.ToolError[@tool.NoToolError]) -> String {
  match error {
    Rpc(error) => "rpc:\{Repr(error)}"
    RemoteTool(error) => "remote:\{Repr(error)}"
    Tool(error) =>
      match error {

      }
    UnknownToolError(name, payload) => {
      @model.drop_owned_capabilities(payload.value)
      "unknown:\{name}"
    }
    InvalidInput(message) => "input:\{message}"
    MalformedRemoteOutput(message) => "output:\{message}"
  }
}

///|
#derive.agent
struct MoonBitToolReflectionCaller {
  name : String
}

///|
fn MoonBitToolReflectionCaller::new(
  name : String,
) -> MoonBitToolReflectionCaller {
  { name, }
}

///|
pub async fn MoonBitToolReflectionCaller::round_trip(self : Self) -> String {
  ignore(self)
  let tool = @reflection.get_tool_type("moonbit-reflection-test") catch {
    error => return "discovery:\{Repr(error)}"
  }
  let command = tool.command(["echo"]) catch {
    error => return "command:\{Repr(error)}"
  }
  let input = Json::object({ "label": Json::string("moonbit") })
  let packed = match command.pack_json(input) {
    Ok(value) => value
    Err(_) => return "input-error"
  }
  command.input_schema().validate_value(packed) catch {
    error => return "input-validation:\{Repr(error)}"
  }
  let typed_input = command.input_schema().typed_value(packed) catch {
      error => return "input-validation:\{Repr(error)}"
    }
  let invalid = command.pack_json(Json::object({ "label": Json::number(42.0) }))
    is Err(_)
  let native = command.invoke_value(packed)
  let encoded = command.invoke_json(input)
  let dynamic = @reflection.DynamicToolClient::new(tool.lookup_name()).invoke_value(
    command.path(),
    typed_input,
  )
  let typed = @reflection.ToolClientDefinition::unnamed(fn(name) {
    MoonbitReflectionTestClient::new_for(name)
  }).client_for(tool.lookup_name()) catch {
    error => return "typed-definition:\{Repr(error)}"
  }
  defer typed.drop()
  match typed.echo("moonbit") {
    Ok("moonbit-tool:moonbit") => ()
    Ok(value) => return "typed-value:\{value}"
    Err(error) => return "typed-error:\{no_tool_error(error)}"
  }
  let native_text = match native {
    Ok(Some(@model.SchemaValue::String(value))) => value
    Ok(Some(value)) => {
      @model.drop_owned_capabilities(value)
      "native-error"
    }
    _ => "native-error"
  }
  let json_text = match encoded {
    Ok(Some(value)) => value.stringify()
    _ => "json-error"
  }
  let dynamic_text = match dynamic {
    Ok(result) =>
      match (command.result(), result.result) {
        (Some(schema), Some(value)) => {
          schema.validate_value(value.value) catch {
            _ => {
              @model.drop_owned_capabilities(value.value)
              return "dynamic-validation-error"
            }
          }
          let decoded = schema.unpack_json(value.value) catch {
            _ => {
              @model.drop_owned_capabilities(value.value)
              return "dynamic-decode-error"
            }
          }
          @model.drop_owned_capabilities(value.value)
          match decoded {
            Json::String(text) => text
            _ => "dynamic-value-error"
          }
        }
        (None, None) => "dynamic-unit"
        (_, Some(value)) => {
          @model.drop_owned_capabilities(value.value)
          "dynamic-unexpected"
        }
        _ => "dynamic-empty"
      }
    Err(error) => "dynamic-error:\{reflected_tool_error(error)}"
  }
  "\{native_text}|\{json_text}|\{invalid}|\{dynamic_text}"
}

///|
pub async fn MoonBitToolReflectionCaller::optional_round_trip(
  self : Self,
) -> String {
  ignore(self)
  let tool = @reflection.get_tool_type("moonbit-reflection-test") catch {
    error => return "discovery:\{Repr(error)}"
  }
  let command = tool.command(["optional"]) catch {
    error => return "command:\{Repr(error)}"
  }
  let omitted_json = command.invoke_json(Json::object({}))
  let null_json = command.invoke_json(Json::object({ "maybe": Json::null() }))
  let supplied_json = command.invoke_json(
    Json::object({ "maybe": Json::string("supplied") }),
  )
  let omitted_native = command.invoke_value(
    @model.SchemaValue::Record([@model.SchemaValue::Option(None)]),
  )
  let supplied_native = command.invoke_value(
    @model.SchemaValue::Record([
      @model.SchemaValue::Option(Some(@model.SchemaValue::String("supplied"))),
    ]),
  )
  let text_json = fn(
    value : Result[Json?, @tool.ToolError[@reflection.ReflectedToolCustomError]],
  ) -> String {
    match value {
      Ok(Some(Json::String(text))) => text
      Err(error) => "json-error:\{reflected_tool_error(error)}"
      _ => "json-error:unexpected"
    }
  }
  let text_native = fn(
    value : Result[
      @model.SchemaValue?,
      @tool.ToolError[@reflection.ReflectedToolCustomError],
    ],
  ) -> String {
    match value {
      Ok(Some(@model.SchemaValue::String(text))) => text
      Err(error) => "native-error:\{reflected_tool_error(error)}"
      _ => "native-error:unexpected"
    }
  }
  let generated = MoonbitReflectionTestClient::new()
  defer generated.drop()
  let generated_omitted = match generated.optional(None) {
    Ok(value) => value
    Err(_) => return "generated-omitted-error"
  }
  let generated_supplied = match generated.optional(Some("supplied")) {
    Ok(value) => value
    Err(_) => return "generated-supplied-error"
  }
  "\{text_json(omitted_json)}|\{text_json(null_json)}|\{text_json(supplied_json)}|\{text_native(omitted_native)}|\{text_native(supplied_native)}|\{generated_omitted}|\{generated_supplied}"
}

///|
pub async fn MoonBitToolReflectionCaller::canonical_round_trip(
  self : Self,
) -> String {
  ignore(self)
  let tool = @reflection.get_tool_type("moonbit-reflection-test") catch {
    error => return "discovery:\{Repr(error)}"
  }
  let command = tool.command(["canonical-values"]) catch {
    error => return "command:\{Repr(error)}"
  }
  let result = command.invoke_json(
    Json::object({
      "signed": Json::string("-9223372036854775808"),
      "unsigned": Json::string("18446744073709551615"),
      "duration": Json::object({
        "nanoseconds": Json::string("9223372036854775807"),
      }),
      "quantity": Json::object({
        "mantissa": Json::string("-9223372036854775808"),
        "scale": Json::number(-9.0),
        "unit": Json::string("m"),
      }),
    }),
  )
  match result {
    Ok(Some(Json::String(text))) => text
    Ok(_) => "canonical-error:unexpected"
    Err(error) => "canonical-error:\{reflected_tool_error(error)}"
  }
}

///|
pub async fn MoonBitToolReflectionCaller::principal_round_trip(
  self : Self,
) -> String {
  let name = "principal-" + self.name
  let agent_type = match @reflection.get_agent_type("AuditLog") {
    Some(value) => value
    None => return "missing-audit-log"
  }
  let reflected = agent_type.get_json(
    Json::object({ "name": Json::string(name) }),
  ) catch {
    error => return "reflected-binding:\{Repr(error)}"
  }
  defer reflected.drop()
  let reflected_result = reflected.invoke_json("get_creator", Json::object({})) catch {
    error => return "reflected-call:\{Repr(error)}"
  }
  let parts = reflected_result.metadata.agent_id.parts() catch {
    error => return "id:\{Repr(error)}"
  }
  guard parts.constructor_value ==
    @model.SchemaValue::Record([@model.SchemaValue::String(name)]) else {
    return "principal-in-id"
  }
  let full = @reflection.FullAgentClient::bind(
    "AuditLog",
    reflected_result.metadata.agent_id,
    (@schema.type_tag() : @schema.TypeTag[AuditLogConstructor]),
  ) catch {
    error => return "full-binding:\{Repr(error)}"
  }
  defer full.drop()
  let method_only = @reflection.MethodOnlyAgentClient::bind(
    reflected_result.metadata.agent_id,
  ) catch {
    error => return "method-binding:\{Repr(error)}"
  }
  defer method_only.drop()
  let full_result : @reflection.Invocation[String] = full.invoke(
    "get_creator",
    EmptyAgentInput::{ },
  ) catch {
    error => return "full-call:\{Repr(error)}"
  }
  let method_result : @reflection.Invocation[String] = method_only.invoke(
    "get_creator",
    EmptyAgentInput::{ },
  ) catch {
    error => return "method-call:\{Repr(error)}"
  }
  let generated = AuditLogClient::get(name) catch {
    error => return "generated-binding:\{Repr(error)}"
  }
  defer generated.drop()
  let generated_id = generated.get_agent_id() catch {
    error => return "generated-id:\{Repr(error)}"
  }
  guard full_result.metadata.agent_id == reflected_result.metadata.agent_id &&
    method_result.metadata.agent_id == reflected_result.metadata.agent_id &&
    generated_id == reflected_result.metadata.agent_id.value else {
    return "identity-mismatch"
  }
  let generated_result = generated.get_creator()
  let reflected_text = match reflected_result.value {
    Some(Json::String(value)) => value
    _ => return "reflected-output"
  }
  "\{reflected_text}|\{full_result.value}|\{method_result.value}|\{generated_result}"
}

///|
pub async fn MoonBitToolReflectionCaller::agent_round_trip(
  self : Self,
) -> String {
  let agent_type = match @reflection.get_agent_type("Counter") {
    Some(value) => value
    None => return "missing-counter"
  }
  let client = agent_type.get_json(
    Json::object({ "name": Json::string("reflected-" + self.name) }),
  ) catch {
    error => return "binding:\{Repr(error)}"
  }
  let first = client.invoke_json("get_value", Json::object({})) catch {
    error => return "first:\{Repr(error)}"
  }
  let native = client.invoke_value("get_value", @model.SchemaValue::Record([])) catch {
    error => return "native:\{Repr(error)}"
  }
  let parts = first.metadata.agent_id.parts() catch {
    error => return "identity:\{Repr(error)}"
  }
  let discovered = @reflection.get_agent_type_by_agent_id(
    first.metadata.agent_id,
  ) catch {
    error => return "lookup:\{Repr(error)}"
  }
  ignore(
    client.invoke_json("increment", Json::object({})) catch {
      error => return "increment:\{Repr(error)}"
    },
  )
  let second = client.invoke_json("get_value", Json::object({})) catch {
    error => return "second:\{Repr(error)}"
  }
  let dynamic = @reflection.DynamicAgentClient::from_agent_id(
    first.metadata.agent_id,
  ) catch {
    error => return "dynamic-binding:\{Repr(error)}"
  }
  let third = dynamic.invoke_value("get_value", @model.SchemaValue::Record([])) catch {
    error => return "dynamic:\{Repr(error)}"
  }
  let first_value = match first.value {
    Some(value) => value.stringify()
    None => "unit"
  }
  let second_value = match second.value {
    Some(value) => value.stringify()
    None => "unit"
  }
  let native_value = match native.value {
    Some(@model.SchemaValue::U64(value)) => "\{value}"
    _ => "unexpected"
  }
  let dynamic_value = match third.value {
    Some(@model.SchemaValue::U64(value)) => "\{value}"
    _ => "unexpected"
  }
  let discovered_name = match discovered {
    Some(value) => value.name
    None => "missing"
  }
  "\{parts.type_name}|\{discovered_name}|\{first_value}|\{native_value}|\{second_value}|\{dynamic_value}"
}
