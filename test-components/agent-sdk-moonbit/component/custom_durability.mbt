///|
/// Exercises custom durability scopes across concurrent coroutines of one invocation.
#derive.agent
struct CustomDurability {
  name : String
}

///|
fn CustomDurability::new(name : String) -> CustomDurability {
  { name, }
}

///|
let port : Ref[String] = Ref("9999")

///|
/// Reads the callback server port once, before any concurrent work starts.
pub fn CustomDurability::warmup(self : Self) -> Unit {
  ignore(self.name)
  for entry in @environment.get_environment() {
    if entry.0 == "PORT" {
      port.val = entry.1
    }
  }
}

///|
/// An ordinary host call outside any custom scope.
pub async fn CustomDurability::callback(
  self : Self,
  payload : String,
) -> String {
  ignore(self.name)
  callback(payload)
}

///|
/// A parent scope with a nested scope and an own effect, then a root call.
pub async fn CustomDurability::nested(self : Self) -> Array[String] {
  ignore(self.name)
  let outer = @api.durable_async(
    "golem-it",
    "test-generator-parent",
    @api.WriteRemote,
    (),
    async fn() {
      let inner = generator("nested")
      let own = callback("parent-own")
      inner + "+" + own
    },
  )
  let root = callback("root-sibling")
  [outer, root]
}

///|
/// Two identical sibling scopes and a root sibling that runs while both wait.
pub async fn CustomDurability::concurrent(self : Self) -> Array[String] {
  ignore(self.name)
  @asyncCore.with_task_group(async fn(group) {
    let slow = group.spawn(async fn() { generator("slow") })
    let fast = group.spawn(async fn() { generator("fast") })
    let root = group.spawn(async fn() { callback("root") })
    [slow.wait(), fast.wait(), root.wait()]
  })
}

///|
/// The first sibling completes and then makes a root call while the second
/// sibling's scope is still open.
pub async fn CustomDurability::first_finishes(self : Self) -> Array[String] {
  ignore(self.name)
  @asyncCore.with_task_group(async fn(group) {
    let first = group.spawn(async fn() {
      let first = generator("first")
      ignore(request(@httpTypes.Method::Get, "/release", None))
      first
    })
    let second = group.spawn(async fn() { generator("second") })
    [first.wait(), second.wait()]
  })
}

///|
/// A scope whose body fails, a scope cancelled by a failing sibling that made
/// a root call while the scope was open, and a root call after both.
pub async fn CustomDurability::failures(self : Self) -> Array[String] {
  ignore(self.name)
  let failed = @api.durable_async(
    "golem-it",
    "test-failing",
    @api.WriteRemote,
    (),
    async fn() -> String {
      ignore(callback("fail-body"))
      fail("the body fails")
    },
  ) catch {
    _ => "caught"
  }
  let cancelled = @asyncCore.with_task_group(async fn(group) {
    let (opened, signal) : (@asyncCore.Stream[Byte], @asyncCore.Sink[Byte]) = @asyncCore.Stream::new(
      capacity=1,
    )
    group.spawn_bg(async fn() {
      ignore(
        @api.durable_async("golem-it", "test-generator", @api.WriteRemote, (), async fn() {
          ignore(signal.write_all_bytes(b"o"[:]))
          callback("hang")
        }),
      )
    })
    ignore(opened.read(1))
    ignore(callback("trigger"))
    fail("the sibling fails")
  }) catch {
    _ => "cancelled"
  }
  [failed, cancelled, callback("after-failures")]
}

///|
/// A scope whose body leaves a spawned task running.
pub async fn CustomDurability::escaped(self : Self) -> String {
  ignore(self.name)
  @asyncCore.with_task_group(async fn(group) {
    @api.durable_async("golem-it", "test-escaped", @api.WriteRemote, (), async fn() {
      group.spawn_bg(async fn() { ignore(callback("escaped")) })
      "returned"
    })
  })
}

///|
/// A scope whose request body is produced by a stream producer.
pub async fn CustomDurability::producer(self : Self) -> String {
  ignore(self.name)
  @api.durable_async("golem-it", "test-upload", @api.WriteRemote, (), async fn() {
    let body : @asyncCore.Stream[Byte] = @asyncCore.Stream::produce(async fn(
      sink,
    ) {
      guard sink.write_all_bytes(b"streamed"[:]) else { return }
      sink.close()
    })
    request(@httpTypes.Method::Post, "/echo", Some(body))
  })
}

///|
/// A scope whose request trailers come from a future producer that makes a
/// call after the body has returned, and a root call after the scope.
pub async fn CustomDurability::future_producer(self : Self) -> Array[String] {
  ignore(self.name)
  let upload = @api.durable_async(
    "golem-it",
    "test-trailers",
    @api.WriteRemote,
    (),
    async fn() {
      let (returned, signal) : (@asyncCore.Stream[Byte], @asyncCore.Sink[Byte]) = @asyncCore.Stream::new(
        capacity=1,
      )
      let trailers = @asyncCore.Future::from(async fn() -> Result[
        @httpTypes.Fields?,
        @httpTypes.ErrorCode,
      ] {
        ignore(callback("trailer"))
        ignore(returned.read(1))
        ignore(callback("producer-after-body"))
        Ok(None)
      })
      let response = request(
        @httpTypes.Method::Post,
        "/early",
        None,
        trailers~,
        await_transmission=false,
      )
      ignore(callback("body-returned"))
      // The write fits the buffer, so the body returns before the producer resumes.
      ignore(signal.write_all_bytes(b"r"[:]))
      response
    },
  )
  [upload, callback("after-producer")]
}

///|
/// A scope cancelled by a failing sibling while it finishes, and a root call
/// after the group. The result names the phase the scope was in when the
/// sibling failed.
pub async fn CustomDurability::cancel_finish(self : Self) -> Array[String] {
  ignore(self.name)
  let body_returned = Ref(false)
  let finish_returned = Ref(false)
  let phase = Ref("not-observed")
  let cancelled = @asyncCore.with_task_group(async fn(group) {
    let (finishing, signal) : (@asyncCore.Stream[Byte], @asyncCore.Sink[Byte]) = @asyncCore.Stream::new(
      capacity=1,
    )
    group.spawn_bg(async fn() {
      ignore(
        @api.durable_async(
          "golem-it",
          "test-finish-cancelled",
          @api.WriteRemote,
          (),
          async fn() {
            ignore(signal.write_all_bytes(b"f"[:]))
            body_returned.val = true
            "finished"
          },
        ),
      )
      finish_returned.val = true
      // Replay skips a finished body, so the sibling still needs its signal.
      if !body_returned.val {
        ignore(signal.write_all_bytes(b"f"[:]))
      }
    })
    ignore(finishing.read(1))
    phase.val = if finish_returned.val {
      "after-finish"
    } else if body_returned.val {
      "during-finish"
    } else {
      "in-body"
    }
    fail("the sibling fails while the scope finishes")
  }) catch {
    _ => phase.val
  }
  [cancelled, callback("after-finish-cancel")]
}

///|
async fn generator(payload : String) -> String {
  @api.durable_async("golem-it", "test-generator", @api.WriteRemote, (), async fn() {
    callback(payload)
  })
}

///|
async fn callback(payload : String) -> String {
  request(@httpTypes.Method::Get, "/callback?payload=\{payload}", None)
}

///|
async fn request(
  verb : @httpTypes.Method,
  path : String,
  body : @asyncCore.Stream[Byte]?,
  trailers~ : @asyncCore.Future[
    Result[@httpTypes.Fields?, @httpTypes.ErrorCode],
  ] = @asyncCore.Future::ready(Ok(None)),
  await_transmission~ : Bool = true,
) -> String {
  let (request, transmitted) = @httpTypes.Request::new(
    @httpTypes.Fields::fields(),
    body,
    trailers,
    None,
  )
  guard request.set_method(verb) is Ok(_) &&
    request.set_scheme(Some(@httpTypes.Scheme::Http)) is Ok(_) &&
    request.set_authority(Some("localhost:\{port.val}")) is Ok(_) &&
    request.set_path_with_query(Some(path)) is Ok(_) else {
    abort("invalid request \{path}")
  }
  let response = match @httpClient.send(request) {
    Ok(response) => response
    Err(_) => abort("request \{path} failed")
  }
  guard response.get_status_code() == 200U else {
    abort("request \{path} returned \{response.get_status_code()}")
  }
  let (contents, trailers) = response.consume_body(
    @asyncCore.Future::ready(Ok(())),
  )
  let text = StringBuilder()
  while contents.read(4096) is Some(chunk) {
    for byte in chunk {
      text.write_char(byte.to_int().unsafe_to_char())
    }
  }
  contents.drop()
  if trailers.get() is Ok(Some(fields)) {
    fields.drop()
  }
  if await_transmission {
    ignore(transmitted.get())
  } else {
    transmitted.drop()
  }
  text.to_string()
}
