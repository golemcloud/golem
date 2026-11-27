///|
#derive.config
pub(all) struct DatabaseConfig {
  #derive.config(default="\"localhost\"")
  host : String
  #derive.config(default="5432")
  port : UInt32
  #derive.config(default="\"30\"")
  timeout : UInt64
}

///|
#derive.config
pub(all) struct AppConfig {
  #derive.config(default="\"example\"")
  app_name : String
  #derive.config(default="false")
  debug : Bool
  api_key : @config.Secret[String]
  db : DatabaseConfig
}

///|
/// An agent that demonstrates code-first configuration
#derive.agent
struct ConfiguredAgent {
  config : @config.Config[AppConfig]
  mut request_count : UInt64
}

///|
/// Creates a new configured agent
fn ConfiguredAgent::new(config : @config.Config[AppConfig]) -> ConfiguredAgent {
  { config, request_count: 0, }
}

///|
/// Returns the captured static application name and database address.
pub fn ConfiguredAgent::settings(self : Self) -> String {
  let config = self.config.value
  "\{config.app_name}:\{config.db.host}:\{config.db.port}"
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
