///|
#derive.tool("moonbit-streaming", version="1.0.0")
struct MoonBitStreaming {}

///|
pub async fn MoonBitStreaming::stream(
  mode : String,
  stdin : @asyncCore.Stream[Byte],
  stdout : @tool.ProviderStdout,
) -> UInt64 {
  if mode == "marker-echo" {
    guard (stdout.write_all_bytes(b"moonbit-marker:") catch { _ => false }) else {
      abort("MoonBit marker write failed")
    }
  }
  let mut bytes_read = 0UL
  for ;; {
    match (stdin.read(65536) catch { _ => None }) {
      None => break
      Some(chunk) => {
        bytes_read += chunk.length().to_uint64()
        guard (stdout.write_all(chunk) catch { _ => false }) else {
          abort("MoonBit echo write failed")
        }
      }
    }
  }
  bytes_read
}

///|
#derive.agent
struct MoonBitToolStreamingCaller {
  name : String
}

///|
fn MoonBitToolStreamingCaller::new(name : String) -> MoonBitToolStreamingCaller {
  { name, }
}

///|
pub async fn MoonBitToolStreamingCaller::marker_before_eof(
  self : Self,
  payload : Bytes,
) -> UInt64 {
  ignore(self)
  let (release, release_input) = @asyncCore.Future::new()
  let stdin = @asyncCore.Stream::produce(async fn(sink) {
    release.get()
    guard sink.write_all_bytes(payload[:]) else {
      abort("MoonBit input stream closed before payload")
    }
  })
  let client = MoonbitStreamingClient::new()
  defer client.drop()
  let invocation = match client.stream("marker-echo", stdin) {
    Ok(invocation) => invocation
    Err(_) => abort("failed to start MoonBit streaming tool")
  }
  let marker = match invocation.stdout.read(65536) {
    Some(marker) => Bytes::from_array(marker)
    None => abort("MoonBit stdout closed before live marker")
  }
  guard marker == b"moonbit-marker:" else {
    abort("unexpected MoonBit live marker")
  }
  ignore(release_input.complete(()))
  match invocation.collect() {
    Ok((bytes_read, output)) => {
      guard output == payload else { abort("MoonBit echoed payload differs") }
      bytes_read
    }
    Err(_) => abort("MoonBit streaming tool failed")
  }
}
