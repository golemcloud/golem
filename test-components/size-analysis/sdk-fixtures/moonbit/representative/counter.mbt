///|
let payload : String = "__SIZE_PAYLOAD__"

///|
#derive.config
struct Settings {
  label : String
}

///|
#derive.agent
#derive.mount("/counters/{name}")
struct CounterAgent {
  name : String
  config : @config.Config[Settings]
  mut count : UInt64
}

///|
fn CounterAgent::new(name : String, config : @config.Config[Settings]) -> CounterAgent {
  { name, config, count: 0 }
}

///|
#derive.endpoint(get="/value")
pub fn CounterAgent::value(self : Self) -> UInt64 {
  self.count
}

///|
pub fn CounterAgent::add(self : Self, by : UInt64) -> UInt64 {
  self.count += by
  self.count
}

///|
pub fn CounterAgent::configured(self : Self) -> String {
  self.config.value.label
}

///|
pub fn CounterAgent::payload_length(_self : Self) -> Int {
  payload.length()
}

///|
pub fn CounterAgent::slice(_self : Self, start : Int, length : Int) -> String {
  (try! payload[start:start + length]).to_string()
}

///|
pub fn CounterAgent::dependency(self : Self, start : Int, length : Int) -> String {
  @crypto.bytes_to_hex_string(@crypto.sha256(@utf8.encode(self.slice(start, length))))
}

///|
pub async fn CounterAgent::rpc_add(self : Self, by : UInt64) -> UInt64 {
  let client = SizeHelperClient::upsert(self.name)
  defer client.drop()
  client.add(by)
}

///|
pub async fn CounterAgent::tool(_self : Self) -> UInt64 {
  let client = @provider.MoonbitStreamingClient::new()
  defer client.drop()
  match client.no_streams() {
    Ok(value) => value
    Err(_) => abort("size fixture tool call failed")
  }
}

///|
#derive.agent
struct SizeHelper {
  name : String
  mut count : UInt64
}

///|
fn SizeHelper::new(name : String) -> SizeHelper {
  { name, count: 0 }
}

///|
pub fn SizeHelper::value(self : Self) -> UInt64 {
  self.count
}

///|
pub fn SizeHelper::add(self : Self, by : UInt64) -> UInt64 {
  self.count += by
  self.count
}
