///|
#derive.golem_schema
enum LifecycleError {
  #derive.error(kind="runtime-error", exit_code="17")
  Expected(String)
}

///|
#derive.tool("moonbit-lifecycle-gol40", version="1.0.0")
struct MoonBitLifecycleGol40 {}

///|
pub async fn MoonBitLifecycleGol40::complete(
  stdout : @tool.ProviderOutput,
) -> Result[UInt64, LifecycleError] {
  guard stdout.write_all_bytes(b"complete:\x00\xff") else {
    abort("complete write")
  }
  41UL |> Ok
}

///|
pub async fn MoonBitLifecycleGol40::declared(
  stdout : @tool.ProviderOutput,
) -> Result[UInt64, LifecycleError] {
  guard stdout.write_all_bytes(b"declared:\x80") else {
    abort("declared write")
  }
  Err(Expected("planned"))
}

///|
pub async fn MoonBitLifecycleGol40::exception(
  stdout : @tool.ProviderOutput,
) -> UInt64 {
  guard stdout.write_all_bytes(b"exception:") else { abort("exception write") }
  abort("moonbit lifecycle exception")
}

///|
pub async fn MoonBitLifecycleGol40::abandon(
  stdout : @tool.ProviderOutput,
) -> UInt64 {
  guard stdout.write_all_bytes(b"abandoned:") else { abort("abandon write") }
  stdout.drop()
  43UL
}

///|
pub async fn MoonBitLifecycleGol40::blocked(
  stdin : @asyncCore.Stream[Byte],
  stdout : @tool.ProviderOutput,
) -> UInt64 {
  guard stdout.write_all_bytes(b"blocked:") else { abort("blocked write") }
  ignore(stdin.read(1))
  47UL
}

///|
pub async fn MoonBitLifecycleGol40::finish_after_input(
  stdin : @asyncCore.Stream[Byte],
  stdout : @tool.ProviderOutput,
) -> String {
  guard stdout.write_all_bytes(b"finish:") else { abort("finish marker") }
  ignore(stdin.read(1))
  try stdout.finish() catch {
    @toolStreams.StreamWriteError::Closed(
      @toolStreams.ByteStreamCloseCause::ConsumerCancelled
    ) => "consumer-cancelled"
    error => "unexpected:" + repr(error)
  } noraise {
    _ => "unexpected-success"
  }
}

///|
pub async fn MoonBitLifecycleGol40::reader_wait(
  stdin : @asyncCore.Stream[Byte],
  stdout : @tool.ProviderOutput,
) -> UInt64 {
  ignore(stdout)
  ignore(stdin.read(1))
  53UL
}

///|
#derive.golem_schema
struct LifecycleObservation {
  bytes : Array[Byte]
  streamTerminal : String
  resultTerminal : String
}

///|
#derive.agent
struct MoonBitLifecycleGol40Caller {
  name : String
}

///|
fn MoonBitLifecycleGol40Caller::new(name : String) -> Self {
  { name, }
}

///|
async fn stream_terminal(stream : @asyncCore.Stream[Byte]) -> String {
  try stream.read(1) catch {
    @tool.ToolStreamError::Cancelled => "cancelled"
    @tool.ToolStreamError::Abandoned => "abandoned"
    @tool.ToolStreamError::ResourceExhausted => "resource-exhausted"
    @tool.ToolStreamError::Failed(message) => "failed:" + message
    error => "unexpected:" + repr(error)
  } noraise {
    None => "finished"
    Some(_) => "data"
  }
}

///|
async fn first_bytes(stream : @asyncCore.Stream[Byte]) -> Bytes {
  match stream.read(65536) {
    Some(chunk) =>
      Bytes::from_array(Array::makei(chunk.length(), i => chunk[i]))
    None => b""
  }
}

///|
pub async fn MoonBitLifecycleGol40Caller::complete(
  self : Self,
) -> LifecycleObservation {
  ignore(self)
  let client = MoonbitLifecycleGol40Client::new()
  defer client.drop()
  let invocation = client.complete().unwrap()
  defer invocation.drop()
  let stdout = invocation.stdout.unwrap()
  let bytes = first_bytes(stdout)
  let streamTerminal = stream_terminal(stdout)
  let resultTerminal = match invocation.get() {
    Ok(41UL) => "ok:41"
    Ok(value) => "unexpected-ok:" + value.to_string()
    Err(_) => "error"
  }
  { bytes: bytes.to_array(), streamTerminal, resultTerminal, }
}

///|
pub async fn MoonBitLifecycleGol40Caller::declared(
  self : Self,
) -> LifecycleObservation {
  ignore(self)
  let client = MoonbitLifecycleGol40Client::new()
  defer client.drop()
  let invocation = client.declared().unwrap()
  defer invocation.drop()
  let stdout = invocation.stdout.unwrap()
  let bytes = first_bytes(stdout)
  let streamTerminal = stream_terminal(stdout)
  let resultTerminal = match invocation.get() {
    Err(@tool.ToolError::Tool(Expected(message))) => "declared:" + message
    Ok(value) => "unexpected-ok:" + value.to_string()
    Err(_) => "unexpected"
  }
  { bytes: bytes.to_array(), streamTerminal, resultTerminal, }
}

///|
pub async fn MoonBitLifecycleGol40Caller::writer_abandonment(
  self : Self,
) -> LifecycleObservation {
  ignore(self)
  let client = MoonbitLifecycleGol40Client::new()
  defer client.drop()
  let invocation = client.abandon().unwrap()
  defer invocation.drop()
  let stdout = invocation.stdout.unwrap()
  let bytes = first_bytes(stdout)
  let streamTerminal = stream_terminal(stdout)
  let resultTerminal = match invocation.get() {
    Ok(43UL) => "ok:43"
    Ok(value) => "unexpected-ok:" + value.to_string()
    Err(_) => "error"
  }
  { bytes: bytes.to_array(), streamTerminal, resultTerminal, }
}

///|
pub async fn MoonBitLifecycleGol40Caller::exception(
  self : Self,
) -> LifecycleObservation {
  ignore(self)
  let client = MoonbitLifecycleGol40Client::new()
  defer client.drop()
  let invocation = client.exception().unwrap()
  defer invocation.drop()
  let stdout = invocation.stdout.unwrap()
  let bytes = first_bytes(stdout)
  let streamTerminal = stream_terminal(stdout)
  let resultTerminal = match invocation.get() {
    Err(@tool.ToolError::Rpc(@tool.RpcError::RemoteInternal(_))) =>
      "remote-internal"
    Ok(value) => "unexpected-ok:" + value.to_string()
    Err(_) => "unexpected"
  }
  { bytes: bytes.to_array(), streamTerminal, resultTerminal, }
}

///|
pub async fn MoonBitLifecycleGol40Caller::explicit_cancellation(
  self : Self,
) -> LifecycleObservation {
  ignore(self)
  let (pending, promise) = @asyncCore.Future::new()
  let stdin = @asyncCore.Stream::produce(async fn(_) { pending.get() })
  let client = MoonbitLifecycleGol40Client::new()
  defer client.drop()
  let invocation = client.blocked(stdin).unwrap()
  let stdout = invocation.stdout.unwrap()
  invocation.cancel()
  let streamTerminal = stream_terminal(stdout)
  ignore(promise.complete(()))
  let resultTerminal = match invocation.get() {
    Err(@tool.ToolError::Rpc(@tool.RpcError::Cancelled)) => "cancelled"
    Ok(value) => "unexpected-ok:" + value.to_string()
    Err(_) => "unexpected"
  }
  invocation.drop()
  { bytes: [], streamTerminal, resultTerminal, }
}

///|
pub async fn MoonBitLifecycleGol40Caller::finish_failure(
  self : Self,
) -> LifecycleObservation {
  ignore(self)
  let (release, release_input) = @asyncCore.Future::new()
  let stdin = @asyncCore.Stream::produce(async fn(sink) {
    release.get()
    ignore(sink.write_all([b"x"[0]]))
  })
  let client = MoonbitLifecycleGol40Client::new()
  defer client.drop()
  let invocation = client.finish_after_input(stdin).unwrap()
  defer invocation.drop()
  let stdout = invocation.stdout.unwrap()
  let bytes = first_bytes(stdout)
  stdout.drop()
  ignore(release_input.complete(()))
  let resultTerminal = match invocation.get() {
    Ok(value) => value
    Err(_) => "unexpected-error"
  }
  {
    bytes: bytes.to_array(),
    streamTerminal: "consumer-cancelled",
    resultTerminal,
  }
}

///|
pub async fn MoonBitLifecycleGol40Caller::output_reader_cancellation(
  self : Self,
) -> LifecycleObservation {
  ignore(self)
  let (release, release_input) = @asyncCore.Future::new()
  let stdin = @asyncCore.Stream::produce(async fn(sink) {
    release.get()
    ignore(sink.write_all([b"x"[0]]))
  })
  let client = MoonbitLifecycleGol40Client::new()
  defer client.drop()
  let invocation = client.reader_wait(stdin).unwrap()
  defer invocation.drop()
  let stdout = invocation.stdout.unwrap()
  let readCancelled = @asyncCore.with_task_group(async fn(group) {
    let read = group.spawn(async fn() { stdout.read(1) })
    let (turn, next_turn) = @asyncCore.Future::new()
    group.spawn_bg(fn() { ignore(next_turn.complete(())) })
    turn.get()
    let was_pending = read.try_wait() is None
    read.cancel()
    let cancelled = try read.wait() catch {
      @asyncCore.Cancelled::Cancelled => true
      _ => false
    } noraise {
      _ => false
    }
    was_pending && cancelled
  })
  ignore(release_input.complete(()))
  let resultTerminal = match invocation.get() {
    Ok(53UL) => "ok:53"
    Ok(value) => "unexpected-ok:" + value.to_string()
    Err(_) => "unexpected-error"
  }
  {
    bytes: [],
    streamTerminal: if readCancelled {
      "reader-cancelled"
    } else {
      "unexpected"
    },
    resultTerminal,
  }
}

///|
pub async fn MoonBitLifecycleGol40Caller::dropped_observer(
  self : Self,
) -> LifecycleObservation {
  ignore(self)
  let client = MoonbitLifecycleGol40Client::new()
  defer client.drop()
  let invocation = client.complete().unwrap()
  let stdout = invocation.stdout.unwrap()
  invocation.detach_result_observer()
  let bytes = first_bytes(stdout)
  let streamTerminal = stream_terminal(stdout)
  invocation.drop()
  {
    bytes: bytes.to_array(),
    streamTerminal,
    resultTerminal: "observer-dropped",
  }
}

///|
fn main {

}
