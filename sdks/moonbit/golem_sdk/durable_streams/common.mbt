///|
pub type Checkpoint = @durable-streams.DurableStreamCheckpoint

///|
pub type Transport = @durable-streams.DurableStreamTransport

///|
pub type Receipt = @durable-streams.DurableStreamAppendReceipt

///|
pub type HostError = @durable-streams.DurableStreamError

///|
pub type ErrorKind = @durable-streams.DurableStreamErrorKind

///|
pub suberror DurableStreamError {
  Remote(HostError)
  InvalidState(String)
} derive(Debug)

///|
priv struct Retry {
  max_retries : Int
  mut failures : Int
  mut backoff_ms : UInt64
  mut terminal : HostError?
}

///|
fn Retry::new(max_retries : Int) -> Retry raise {
  guard max_retries >= 0 else {
    raise DurableStreamError::InvalidState("max_retries must be nonnegative")
  }
  { max_retries, failures: 0, backoff_ms: 100, terminal: None, }
}

///|
fn Retry::reset(self : Retry) -> Unit {
  self.failures = 0
  self.backoff_ms = 100
  self.terminal = None
}

///|
fn Retry::check(self : Retry) -> Unit raise {
  if self.terminal is Some(error) {
    raise DurableStreamError::Remote(error)
  }
}

///|
fn is_retryable(error : HostError) -> Bool {
  match error.kind {
    TIMEOUT | TRANSPORT | RATE_LIMITED | UNAVAILABLE => true
    _ => false
  }
}

///|
fn Retry::allow_explicit_attempt(self : Retry) -> Unit {
  if self.terminal is Some(error) && is_retryable(error) {
    self.terminal = None
  }
}

///|
fn Retry::delay(self : Retry, error : HostError) -> UInt64 raise {
  self.check()
  if !is_retryable(error) || self.failures >= self.max_retries {
    self.terminal = Some(error)
    raise DurableStreamError::Remote(error)
  }
  self.failures += 1
  let delay = self.backoff_ms
  self.backoff_ms = if delay >= 15000 { 30000 } else { delay * 2 }
  match error.retry_after_ms {
    Some(after) if after > delay => after
    _ => delay
  }
}

///|
async fn wait_ms(ms : UInt64) -> Unit {
  // Chunk very large Retry-After values without overflowing nanoseconds.
  for remaining = ms; remaining > 0; {
    let chunk : UInt64 = if remaining > 300000 { 300000 } else { remaining }
    @clock.wait_for(chunk * 1000000)
    continue remaining - chunk
  }
}

///|
fn check_timeout(timeout_ms : UInt64) -> Unit raise {
  guard timeout_ms > 0 && timeout_ms <= 300000 else {
    raise DurableStreamError::InvalidState("timeout_ms must be in 1..=300000")
  }
}
