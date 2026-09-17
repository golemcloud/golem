///|
#derive.tool("moonbit-reflection-test")
struct MoonBitReflectionTest {}

///|
pub fn MoonBitReflectionTest::echo(label : String) -> String {
  "moonbit-tool:" + label
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
  let dynamic_value = match third.value {
    Some(@model.SchemaValue::U64(value)) => "\{value}"
    _ => "unexpected"
  }
  let discovered_name = match discovered {
    Some(value) => value.name
    None => "missing"
  }
  "\{parts.type_name}|\{discovered_name}|\{first_value}|\{second_value}|\{dynamic_value}"
}
