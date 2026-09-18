///|
/// A serialized, non-pipelining producer. An uncertain request is immutable and
/// blocks different data until retry_pending acknowledges that exact request.
pub struct Writer[T] {
  priv url : String
  priv content_type : String
  priv encode : (Array[T]) -> @host.DurableStreamAppendPayload raise
  priv auth : @types.Secret?
  priv timeout_ms : UInt64
  priv mut producer : @host.DurableStreamProducer
  priv mut pending : @host.DurableStreamAppendRequest?
  priv mut closed : Bool
  priv retry : Retry
  priv lock : @async-core.Mutex
  priv append_batch : async (@host.DurableStreamAppendRequest, @types.Secret?) -> Result[
    Receipt,
    HostError,
  ]
  priv wait : async (UInt64) -> Unit
}

///|
fn[T] make_writer(
  url : String,
  content_type : String,
  encode : (Array[T]) -> @host.DurableStreamAppendPayload raise,
  producer_id : String?,
  epoch : UInt64,
  auth : @types.Secret?,
  timeout_ms : UInt64,
  max_retries : Int,
) -> Writer[T] raise {
  check_timeout(timeout_ms)
  guard epoch <= 9007199254740991 else {
    raise DurableStreamError::InvalidState("producer epoch exceeds 2^53-1")
  }
  let retry = Retry::new(max_retries)
  let id = match producer_id {
    Some(id) => id
    None => @api.generate_idempotency_key_string()
  }
  guard !id.is_empty() else {
    raise DurableStreamError::InvalidState("producer ID must not be empty")
  }
  {
    url,
    content_type,
    encode,
    auth,
    timeout_ms,
    producer: { id, epoch, sequence: 0, },
    pending: None,
    closed: false,
    retry,
    lock: @async-core.Mutex(),
    append_batch: @host.append_durable_stream_batch,
    wait: wait_ms,
  }
}

///|
/// Uses MoonBit's ToJson convention, including string-encoded 64-bit integers.
pub fn[T : ToJson] json_writer(
  url : String,
  producer_id? : String,
  epoch? : UInt64 = 0,
  auth? : @types.Secret,
  timeout_ms? : UInt64 = 30000,
  max_retries? : Int = 8,
) -> Writer[T] raise {
  json_writer_with_codec(
    url,
    value => value.to_json().stringify(),
    producer_id?,
    epoch~,
    auth?,
    timeout_ms~,
    max_retries~,
  )
}

///|
/// Encode each logical message separately. A codec returning an array encodes
/// one array-valued message; only the host adds the outer array framing.
pub fn[T] json_writer_with_codec(
  url : String,
  encode : (T) -> String raise,
  producer_id? : String,
  epoch? : UInt64 = 0,
  auth? : @types.Secret,
  timeout_ms? : UInt64 = 30000,
  max_retries? : Int = 8,
) -> Writer[T] raise {
  make_writer(
    url,
    "application/json",
    values => {
      let encoded = values.map(value => {
        let text = encode(value)
        // Validate, but never re-encode or round the caller's numeric lexemes.
        ignore(@json.parse(text))
        text
      })
      @host.DurableStreamAppendPayload::Json(encoded)
    },
    producer_id,
    epoch,
    auth,
    timeout_ms,
    max_retries,
  )
}

///|
pub fn byte_writer(
  url : String,
  content_type? : String = "application/octet-stream",
  producer_id? : String,
  epoch? : UInt64 = 0,
  auth? : @types.Secret,
  timeout_ms? : UInt64 = 30000,
  max_retries? : Int = 8,
) -> Writer[Byte] raise {
  make_writer(
    url,
    content_type,
    values => {
      @host.DurableStreamAppendPayload::Bytes(FixedArray::from_array(values))
    },
    producer_id,
    epoch,
    auth,
    timeout_ms,
    max_retries,
  )
}

///|
fn[T] Writer::prepare(
  self : Writer[T],
  values : Array[T],
  close : Bool,
) -> Unit raise {
  guard self.pending is None else {
    raise DurableStreamError::InvalidState(
      "resolve the pending append with retry_pending first",
    )
  }
  guard !self.closed else {
    raise DurableStreamError::InvalidState("writer is closed")
  }
  guard values.length() > 0 || close else {
    raise DurableStreamError::InvalidState("empty append requires close")
  }
  guard self.producer.sequence <= 9007199254740991 else {
    raise DurableStreamError::InvalidState("producer sequence exhausted")
  }
  let payload = (self.encode)(values)
  self.pending = Some({
    url: self.url,
    content_type: self.content_type,
    payload,
    producer: self.producer,
    close,
    timeout_ms: self.timeout_ms,
  })
}

///|
fn[T] Writer::acknowledge(self : Writer[T], receipt : Receipt) -> Unit raise {
  guard self.pending is Some(request) else {
    raise DurableStreamError::InvalidState("no pending append")
  }
  if receipt.epoch != request.producer.epoch ||
    receipt.sequence != request.producer.sequence ||
    (request.close && !receipt.closed) {
    let kind : ErrorKind = if receipt.epoch == request.producer.epoch &&
      receipt.sequence > request.producer.sequence {
      PRODUCER_DIVERGED
    } else {
      PROTOCOL_ERROR
    }
    let error : HostError = {
      kind,
      message: "append receipt does not match pending producer",
      retry_after_ms: None,
      producer_epoch: Some(receipt.epoch),
      expected_sequence: Some(receipt.sequence),
    }
    self.retry.terminal = Some(error)
    raise DurableStreamError::Remote(error)
  }
  self.producer = {
    ..request.producer,
    sequence: request.producer.sequence + 1,
  }
  self.closed = receipt.closed
  self.pending = None
  self.retry.reset()
}

///|
async fn[T] Writer::send_pending(self : Writer[T]) -> Receipt {
  guard self.pending is Some(request) else {
    raise DurableStreamError::InvalidState("no pending append")
  }
  for ;; {
    self.retry.check()
    match (self.append_batch)(request, self.auth) {
      Ok(receipt) => {
        self.acknowledge(receipt)
        return receipt
      }
      Err(error) => (self.wait)(self.retry.delay(error))
    }
  }
}

///|
/// Assign immutable body, producer tuple and close flag before awaiting the
/// host. Cancellation retains that request; different data cannot replace it.
pub async fn[T] Writer::append(
  self : Writer[T],
  values : Array[T],
  close? : Bool = false,
) -> Receipt {
  self.lock.acquire()
  defer self.lock.release()
  self.prepare(values, close)
  self.send_pending()
}

///|
pub async fn Writer::append_bytes(
  self : Writer[Byte],
  bytes : Bytes,
  close? : Bool = false,
) -> Receipt {
  self.append(bytes.to_array(), close~)
}

///|
/// Close-only consumes one producer sequence, just like append-and-close.
pub async fn[T] Writer::close(self : Writer[T]) -> Receipt {
  self.append([], close=true)
}

///|
/// Resume the same uncertain request. Retryable exhaustion permits one fresh
/// attempt without resetting the automatic budget; permanent errors stay sticky.
pub async fn[T] Writer::retry_pending(self : Writer[T]) -> Receipt {
  self.lock.acquire()
  defer self.lock.release()
  self.retry.allow_explicit_attempt()
  self.send_pending()
}

///|
pub fn[T] Writer::has_pending(self : Writer[T]) -> Bool {
  self.pending is Some(_)
}
