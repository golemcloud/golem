///|
/// The WIT `error` case the fake host returns from send and close, or -1 for success.
extern "wasm" fn set_host_error(error_case : Int32) = "golem:test" "set-websocket-host-error"

///|
/// The policy of the connect at `index` (negative from the end): -1 for none,
/// otherwise the WIT enum ordinal.
extern "wasm" fn connect_policy(index : Int32) -> Int32 = "golem:test" "websocket-connect-policy"

///|
test "connect without a policy sends none, so the host default applies" {
  connect("wss://peer").drop()
  assert_eq(connect_policy(-1), -1)
  connect("wss://peer", reconstruction_policy=ReconnectAutomatically).drop()
  assert_eq(connect_policy(-1), 0)
  connect("wss://peer", reconstruction_policy=ReportConnectionLoss).drop()
  assert_eq(connect_policy(-1), 1)
}

///|
fn failure(operation : () -> Unit raise WebsocketError) -> WebsocketError? {
  try operation() catch {
    error => Some(error)
  } noraise {
    _ => None
  }
}

///|
test "session loss is its own typed error, apart from a peer close" {
  let connection = connect("wss://peer")
  defer connection.drop()
  defer set_host_error(-1)
  set_host_error(6)
  assert_eq(failure(() => connection.send(Text("hello"))), Some(SessionLost))
  assert_eq(failure(() => connection.close()), Some(SessionLost))
  set_host_error(4)
  assert_eq(failure(() => connection.send(Binary(b"\x01"))), Some(Closed(None)))
  set_host_error(-1)
  assert_eq(failure(() => connection.send(Text("hello"))), None)
}
