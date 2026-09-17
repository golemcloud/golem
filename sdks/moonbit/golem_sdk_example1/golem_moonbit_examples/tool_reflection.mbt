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
