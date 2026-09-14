///|
enum FacadeError {
  Rejected
}

///|
impl @tool.ToolErrorSchema for FacadeError with fn error_cases() {
  Ok([])
}

///|
impl @tool.ToolErrorSchema for FacadeError with fn to_error_payload_value(_) {
  Err("not used by the facade check")
}

///|
impl @tool.ToolErrorSchema for FacadeError with fn from_error_payload_value(_) {
  Ok(Rejected)
}

///|
extend FacadeError with @tool.ToolErrorSchema::{error_cases}

///|
test "ambient tool facade preserves nominal source syntax after core split" {
  let root : @tool.CommandNodeDef = {
    name: "facade",
    aliases: [],
    doc: { summary: "facade", description: "", examples: [], },
    globals: { options: [], flags: [], },
    subcommands: [],
    body: None,
  }
  let def : @tool.ToolDef = { version: "1.0.0", commands: [root], }
  assert_true(def.tool_name() is Ok("facade"))
  let cases : Result[Array[@tool.ErrorCaseDef], @tool.ToolBuildError] = FacadeError::error_cases()
  assert_true(cases is Ok([]))
  let literal : @tool.ToolLiteral = @tool.ToolLiteral::Scalar("value")
  assert_true(literal is @tool.ToolLiteral::Scalar("value"))
  let _principal : @tool.Principal? = None
  let _wire_error : @toolCommon.ToolError? = None
  let _model_value : @model.TypedSchemaValue? = None
}
