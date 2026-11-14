///|
#derive.agent
struct MoonBitMethodFailure {
  mut count : Int64
}

///|
fn MoonBitMethodFailure::new(
  name : String,
) -> MoonBitMethodFailure raise @common.AgentError {
  if name == "fail-constructor" {
    raise @common.AgentError::InvalidInput("constructor gave up")
  }
  { count: 0L, }
}

///|
pub fn MoonBitMethodFailure::fail(
  self : Self,
) -> Int64 raise @common.AgentError {
  self.count += 7L
  raise @common.AgentError::InvalidInput("method gave up")
}

///|
pub fn MoonBitMethodFailure::value(self : Self) -> Int64 {
  self.count
}

///|
pub fn MoonBitMethodFailure::increment(self : Self) -> Int64 {
  self.count += 1L
  self.count
}

///|
pub fn MoonBitMethodFailure::declared_error(
  self : Self,
) -> Result[Int64, String] {
  ignore(self)
  Err("declared error")
}
