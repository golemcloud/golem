///|
/// Returns the message an error of this tool carries, for printing to the
/// user. An error from elsewhere is shown as it is.
pub fn error_message(error : Error) -> String {
  match error {
    AgentParseError(message) => message
    ComponentRoleError(message) => message
    HttpPathParseError(message) => message
    ToolEmitError(message) => message
    ToolMiddlewareEmitError(message) => message
    ToolMiddlewareParseError(message) => message
    ToolParseError(message) => message
    other => "\{other}"
  }
}
