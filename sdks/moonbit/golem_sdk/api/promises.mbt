///|
pub using @types {type PromiseId}

///|
/// A handle to the result of a promise. Wraps the raw WIT resource,
/// providing idiomatic access to poll/get operations.
pub(all) struct PromiseResult {
  priv inner : @raw.GetPromiseResult
}

///|
/// Creates a new promise.
pub fn create_promise() -> PromiseId {
  @raw.create_promise()
}

///|
/// Gets a handle to the result of a promise.
/// Can only be called in the same agent that originally created the promise.
pub fn get_promise(promise_id : PromiseId) -> PromiseResult {
  { inner: @raw.get_promise(promise_id) }
}

///|
/// Completes the given promise with the given payload. Returns true if the
/// promise was completed, false if it was already completed. The payload is
/// passed to the agent that is awaiting the promise.
pub fn complete_promise(promise_id : PromiseId, data : Bytes) -> Bool {
  let fixed : FixedArray[Byte] = FixedArray::make(data.length(), b'\x00')
  for i = 0; i < data.length(); i = i + 1 {
    fixed[i] = data[i]
  }
  @raw.complete_promise(promise_id, fixed)
}

///|
/// Awaits and returns the result of the promise.
pub async fn PromiseResult::get(self : PromiseResult) -> Bytes {
  Bytes::from_fixedarray(self.inner.get())
}

///|
/// Drops the underlying resource handle.
pub fn PromiseResult::drop(self : PromiseResult) -> Unit {
  self.inner.drop()
}

///|
/// Awaits the promise and returns its payload. The agent is durably suspended
/// by the Golem runtime until the promise is fulfilled.
pub async fn await_promise(promise_id : PromiseId) -> Bytes {
  let promise = get_promise(promise_id)
  let result = promise.get()
  promise.drop()
  result
}
