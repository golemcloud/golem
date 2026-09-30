///|
priv enum Items {
  JsonItems(Array[Json])
  ByteItems(Bytes)
}

///|
fn Items::length(self : Items) -> Int {
  match self {
    JsonItems(items) => items.length()
    ByteItems(items) => items.length()
  }
}

///|
priv struct PendingBatch {
  batch : @durable-streams.DurableStreamBatch
  items : Items
  mut index : Int
}

///|
/// One bounded batch is retained until every item has been delivered. State is
/// reconstructed by ordinary durable execution, not by serializing native handles.
pub struct Reader[T] {
  priv mut request : @durable-streams.DurableStreamReadRequest
  priv mut resource : @durable-streams.DurableStreamReader?
  priv mode : @durable-streams.DurableStreamMode
  priv live_transport : Transport
  priv decode : (Items, Int) -> T raise
  priv retry : Retry
  priv idle_delay_ms : UInt64
  priv mut pending : PendingBatch?
  priv mut failure : Error?
  priv mut closed : Bool
  priv mut transferred : Bool
  priv lock : @async-core.Mutex
  priv read_batch : async (
    @durable-streams.DurableStreamReadRequest,
    @durable-streams.DurableStreamReader,
  ) -> Result[@durable-streams.DurableStreamBatch, HostError]
  priv wait : async (UInt64) -> Unit
}

///|
fn[T] make_reader(
  url : String,
  mode : @durable-streams.DurableStreamMode,
  decode : (Items, Int) -> T raise,
  offset : String,
  cursor : String?,
  transport : Transport,
  auth : @types.Secret?,
  timeout_ms : UInt64,
  max_retries : Int,
  idle_delay_ms : UInt64,
) -> Reader[T] raise {
  check_timeout(timeout_ms)
  let retry = Retry::new(max_retries)
  let resource = @durable-streams.DurableStreamReader::durable_stream_reader(
    { url, mode, timeout_ms, },
    auth,
  )
  {
    request: {
      checkpoint: { offset, cursor, },
      transport: CATCH_UP,
      content_type: None,
    },
    resource: Some(resource),
    mode,
    live_transport: transport,
    decode,
    retry,
    idle_delay_ms,
    pending: None,
    failure: None,
    closed: false,
    transferred: false,
    lock: @async-core.Mutex(),
    read_batch: (request, resource) => resource.read(request),
    wait: wait_ms,
  }
}

///|
/// Decode individual JSON values with MoonBit's FromJson convention (including
/// string-encoded Int64/UInt64). Use json_reader_with_codec for another encoding.
pub fn[T : @json.FromJson] json_reader(
  url : String,
  offset? : String = "-1",
  cursor? : String,
  transport? : Transport = LONG_POLL,
  auth? : @types.Secret,
  timeout_ms? : UInt64 = 30000,
  max_retries? : Int = 8,
  idle_delay_ms? : UInt64 = 100,
) -> Reader[T] raise {
  json_reader_with_codec(
    url,
    text => @json.from_json(@json.parse(text)),
    offset~,
    cursor?,
    transport~,
    auth?,
    timeout_ms~,
    max_retries~,
    idle_delay_ms~,
  )
}

///|
/// Each codec input is one complete JSON value, never the host's outer array.
/// Numeric lexemes survive parsing/stringification without Double rounding.
pub fn[T] json_reader_with_codec(
  url : String,
  decode : (String) -> T raise,
  offset? : String = "-1",
  cursor? : String,
  transport? : Transport = LONG_POLL,
  auth? : @types.Secret,
  timeout_ms? : UInt64 = 30000,
  max_retries? : Int = 8,
  idle_delay_ms? : UInt64 = 100,
) -> Reader[T] raise {
  make_reader(
    url,
    JSON,
    (items, index) => {
      match items {
        JsonItems(values) => decode(values[index].stringify())
        ByteItems(_) => abort("JSON reader received byte items")
      }
    },
    offset,
    cursor,
    transport,
    auth,
    timeout_ms,
    max_retries,
    idle_delay_ms,
  )
}

///|
/// A byte sequence; remote append and transport batch boundaries are not items.
pub fn byte_reader(
  url : String,
  offset? : String = "-1",
  cursor? : String,
  transport? : Transport = LONG_POLL,
  auth? : @types.Secret,
  timeout_ms? : UInt64 = 30000,
  max_retries? : Int = 8,
  idle_delay_ms? : UInt64 = 100,
) -> Reader[Byte] raise {
  make_reader(
    url,
    BYTES,
    fn(items, index) {
      match items {
        ByteItems(values) => values[index]
        JsonItems(_) => abort("byte reader received JSON items")
      }
    },
    offset,
    cursor,
    transport,
    auth,
    timeout_ms,
    max_retries,
    idle_delay_ms,
  )
}

///|
fn[T] Reader::install(
  self : Reader[T],
  batch : @durable-streams.DurableStreamBatch,
) -> Unit raise {
  let bytes = Bytes::from_array(batch.payload)
  let items = match self.mode {
    BYTES => ByteItems(bytes)
    JSON => {
      let values : Array[Json] = if bytes.length() == 0 {
        []
      } else {
        @json.from_json(@json.parse(@utf8.decode(bytes)))
      }
      JsonItems(values)
    }
  }
  self.pending = Some({ batch, items, index: 0, })
}

///|
fn[T] Reader::promote(
  self : Reader[T],
  batch : @durable-streams.DurableStreamBatch,
) -> Unit {
  self.request = {
    checkpoint: batch.next,
    content_type: match self.request.content_type {
      Some(content_type) => Some(content_type)
      None => Some(batch.content_type)
    },
    transport: if batch.up_to_date {
      self.live_transport
    } else {
      CATCH_UP
    },
  }
  self.closed = self.closed || batch.closed
  self.pending = None
}

///|
/// The last fully drained checkpoint, not a resumable snapshot of a partial batch.
pub fn[T] Reader::checkpoint(self : Reader[T]) -> Checkpoint {
  self.request.checkpoint
}

///|
fn[T] Reader::release_resource(self : Reader[T]) -> Unit {
  if self.resource is Some(resource) {
    self.resource = None
    resource.drop()
  }
}

///|
/// Prevent further pulls. A pending host import is bounded by timeout_ms; this
/// does not promise immediate interruption of an arbitrary native async import.
pub fn[T] Reader::close(self : Reader[T]) -> Unit {
  self.closed = true
  self.pending = None
  // An in-flight method borrows the handle until the active pull unwinds.
  if self.lock.try_acquire() {
    self.release_resource()
    self.lock.release()
  }
}

///|
async fn[T] Reader::pull(self : Reader[T], from_stream? : Bool = false) -> T? {
  self.lock.acquire()
  defer {
    if self.closed {
      self.release_resource()
    }
    self.lock.release()
  }
  guard from_stream || !self.transferred else {
    raise DurableStreamError::InvalidState("reader transferred to AgentStream")
  }
  if !from_stream {
    self.retry.allow_explicit_attempt()
  }
  for ;; {
    if self.closed {
      return None
    }
    if self.failure is Some(error) {
      raise error
    }
    if self.pending is Some(pending) {
      if pending.index < pending.items.length() {
        let value = (self.decode)(pending.items, pending.index)
        pending.index += 1
        if pending.index == pending.items.length() {
          self.promote(pending.batch)
        }
        return Some(value)
      }
      self.promote(pending.batch)
      if !self.closed && pending.batch.up_to_date {
        (self.wait)(self.idle_delay_ms)
      }
      continue
    }
    self.retry.check()
    let result = (self.read_batch)(self.request, self.resource.unwrap())
    if self.closed {
      return None
    }
    match result {
      Ok(batch) => {
        // No await between delivery and installation of the complete batch.
        self.install(batch) catch {
          error => self.failure = Some(error)
        }
        self.retry.reset()
      }
      Err(error) => (self.wait)(self.retry.delay(error))
    }
  }
}

///|
/// After retryable exhaustion, an explicit read permits one fresh attempt
/// without resetting the automatic failure budget or backoff.
pub async fn[T] Reader::read(self : Reader[T]) -> T? {
  self.pull()
}

///|
/// Transfer this source once to an ordinary AgentStream for local iteration,
/// nested/output values or RPC inputs. Producer failures propagate normally;
/// use a result-valued stream when recoverable item errors are desired.
pub fn[T : @schema.IntoWire + @schema.FromWire] Reader::into_stream(
  self : Reader[T],
) -> @schema.AgentStream[T] raise {
  guard !self.transferred && self.lock.try_acquire() else {
    raise DurableStreamError::InvalidState(
      "reader is busy or already transferred",
    )
  }
  self.lock.release()
  self.transferred = true
  @schema.AgentStream::produce(
    async fn(writer) {
      defer self.close()
      while self.pull(from_stream=true) is Some(item) {
        if writer.write_one(item) is PeerDropped {
          return
        }
      }
    },
    on_unstarted_drop=() => self.close(),
  )
}
