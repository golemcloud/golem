///|
#derive.tool("probe")
struct Probe {}

///|
pub fn Probe::ping() -> String {
  "pong"
}

///|
pub async fn Probe::burst(stdout : @tool.ProviderStdout) -> String {
  for size in [0, 1, 63, 64, 65, 4096, 65537] {
    let bytes = Bytes::from_array([ for i in 0..<size => (i % 251).to_byte() ])
    guard stdout.write_all_bytes(bytes[:]) else { abort("stdout closed") }
  }
  "burst"
}
