///|
#derive.tool("moonbit-reflection-test")
struct MoonBitReflectionTest {}

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
fn reflected_tool_error(
  error : @tool.ToolError[@reflection.ReflectedToolCustomError],
) -> String {
  match error {
    Rpc(error) => "rpc:\{Repr(error)}"
    RemoteTool(error) => "remote:\{Repr(error)}"
    Tool(error) => "tool:\{Repr(error)}"
    UnknownToolError(name, _) => "unknown:\{name}"
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
  let invalid = command.pack_json(Json::object({ "label": Json::number(42.0) }))
    is Err(_)
  let native = command.invoke_value(packed)
  let encoded = command.invoke_json(input)
  let dynamic = @reflection.DynamicToolClient::new("moonbit-reflection-test").invoke_value(
    ["echo"],
    {
      graph: command.input_schema.graph,
      value: @model.SchemaValue::Record([@model.SchemaValue::String("moonbit")]),
    },
  )
  let native_text = match native {
    Ok(Some(@model.SchemaValue::String(value))) => value
    _ => "native-error"
  }
  let json_text = match encoded {
    Ok(Some(value)) => value.stringify()
    _ => "json-error"
  }
  let dynamic_text = match dynamic {
    Ok(result) =>
      match result.result {
        Some(value) =>
          match value.value {
            @model.SchemaValue::String(text) => text
            _ => "dynamic-value-error"
          }
        None => "dynamic-empty"
      }
    Err(_) => "dynamic-error"
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
  let omitted_json = command.invoke_json(
    Json::object({ "maybe": Json::null() }),
  )
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
  "\{text_json(omitted_json)}|\{text_json(supplied_json)}|\{text_native(omitted_native)}|\{text_native(supplied_native)}|\{generated_omitted}|\{generated_supplied}"
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
