///|
/// Converts a string to kebab-case (e.g., "MyAgent" → "my-agent").
pub fn to_kebab_case(s : String) -> String {
  @case.kebab_case(s)
}

///|
/// Constructs the RPC function name for an agent method in the format "agent-name.{method-name}".
pub fn rpc_function_name(agent_name : String, method_name : String) -> String {
  let agent_kebab = to_kebab_case(agent_name)
  let method_kebab = to_kebab_case(method_name)
  "\{agent_kebab}.{\{method_kebab}}"
}
