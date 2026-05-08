///|
#derive.config
pub(all) struct DatabaseConfig {
  host : String
  port : UInt
  timeout : UInt64
}

///|
#derive.config
pub(all) struct AppConfig {
  app_name : String
  debug : Bool
  api_key : @config.Secret[String]
  db : DatabaseConfig
}

///|
/// An agent that demonstrates code-first configuration
#derive.agent
struct ConfiguredAgent {
  mut request_count : UInt64
}

///|
/// Creates a new configured agent
fn ConfiguredAgent::new(config : @config.Config[AppConfig]) -> ConfiguredAgent {
  let _ = config
  { request_count: 0 }
}

///|
/// Returns how many requests have been made
pub fn ConfiguredAgent::get_request_count(self : Self) -> UInt64 {
  self.request_count
}

///|
/// Increments the request counter
pub fn ConfiguredAgent::handle_request(self : Self) -> Unit {
  self.request_count += 1
}
