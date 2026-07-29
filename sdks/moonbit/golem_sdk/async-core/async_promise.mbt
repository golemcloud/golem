// Copyright 2025 International Digital Economy Academy
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

///|
priv enum PromiseState[X] {
  Pending
  Completed(X)
  Failed(Error)
  Closed
  ReaderDropped
  Consumed
}

///|
priv struct PromiseCell[X] {
  mut state : PromiseState[X]
  ready : Semaphore
  cleanup : ((X) -> Unit)?
  mut reading : Bool
}

///|
/// The local producer side of a one-shot `Future[X]`.
///
/// This is a MoonBit coordination value, not a component-model future writable
/// endpoint. Conversion to a component future remains generated at an FFI site.
pub struct Promise[X] {
  priv cell : Ref[PromiseCell[X]]
}

///|
pub(all) suberror PromiseClosed derive(Debug)

///|
fn[X] new_future_promise(cleanup : ((X) -> Unit)?) -> (Future[X], Promise[X]) {
  let cell : Ref[PromiseCell[X]] = {
    val: {
      state: Pending,
      ready: Semaphore(1, initial_value=0),
      cleanup,
      reading: false,
    },
  }
  ({ inner: Promised(cell) }, { cell, })
}

///|
/// Create a local one-shot Future/Promise pair.
pub fn[X] Future::new() -> (Future[X], Promise[X]) {
  new_future_promise(None)
}

///|
/// Create a local one-shot Future/Promise pair with value-discard cleanup.
///
/// The cleanup callback runs when the promise completed successfully but the
/// future is dropped before consuming the value.
pub fn[X] Future::new_with_cleanup(
  cleanup : (X) -> Unit,
) -> (Future[X], Promise[X]) {
  new_future_promise(Some(cleanup))
}

///|
/// Complete the paired future with `value`.
///
/// `true` means the future accepted ownership. `false` means its reader was
/// already dropped and the caller retains ownership of `value`.
pub fn[X] Promise::complete(self : Promise[X], value : X) -> Bool {
  match self.cell.val.state {
    Pending => {
      self.cell.val.state = Completed(value)
      self.cell.val.ready.release()
      true
    }
    ReaderDropped => false
    Completed(_) | Failed(_) | Closed | Consumed =>
      abort("promise already settled")
  }
}

///|
/// Complete the paired future with an error raised by `Future::get`.
pub fn[X] Promise::fail(self : Promise[X], error : Error) -> Bool {
  match self.cell.val.state {
    Pending => {
      self.cell.val.state = Failed(error)
      self.cell.val.ready.release()
      true
    }
    ReaderDropped => false
    Completed(_) | Failed(_) | Closed | Consumed =>
      abort("promise already settled")
  }
}

///|
/// Close the paired future without producing a value.
///
/// A local waiter observes `PromiseClosed`. An outgoing component future still
/// cannot represent this terminal state and follows the bridge's no-value
/// failure policy if it has already crossed an FFI boundary.
pub fn[X] Promise::close(self : Promise[X]) -> Bool {
  match self.cell.val.state {
    Pending => {
      self.cell.val.state = Closed
      self.cell.val.ready.release()
      true
    }
    ReaderDropped => false
    Completed(_) | Failed(_) | Closed | Consumed =>
      abort("promise already settled")
  }
}

///|
fn[X] consume_promised_future(cell : Ref[PromiseCell[X]]) -> X raise {
  match cell.val.state {
    Completed(value) => {
      cell.val.state = Consumed
      value
    }
    Failed(error) => {
      cell.val.state = Consumed
      raise error
    }
    Closed => {
      cell.val.state = Consumed
      raise PromiseClosed
    }
    Pending => panic()
    ReaderDropped => raise Cancelled::Cancelled
    Consumed => abort("future already consumed")
  }
}

///|
async fn[X] promised_future_get(cell : Ref[PromiseCell[X]]) -> X {
  if cell.val.reading {
    abort("future read already in progress")
  }
  match cell.val.state {
    Pending => {
      cell.val.reading = true
      cell.val.ready.acquire() catch {
        err => {
          cell.val.reading = false
          if cell.val.state is Pending {
            cell.val.state = ReaderDropped
          }
          raise err
        }
      }
      cell.val.reading = false
      consume_promised_future(cell)
    }
    Completed(_) | Failed(_) | Closed => consume_promised_future(cell)
    ReaderDropped => consume_promised_future(cell)
    Consumed => abort("future already consumed")
  }
}

///|
fn[X] promised_future_drop(
  cell : Ref[PromiseCell[X]],
  fallback_cleanup : ((X) -> Unit)?,
) -> Unit {
  if cell.val.reading {
    match cell.val.state {
      Pending => {
        cell.val.state = ReaderDropped
        cell.val.ready.release()
      }
      // Once settlement assigned a result, the waiting reader owns it.
      Completed(_) | Failed(_) | Closed => ()
      ReaderDropped | Consumed => ()
    }
    return
  }
  match cell.val.state {
    Pending => cell.val.state = ReaderDropped
    Completed(value) => {
      cell.val.state = Consumed
      match cell.val.cleanup {
        Some(cleanup) => cleanup(value)
        None =>
          match fallback_cleanup {
            Some(cleanup) => cleanup(value)
            None => ()
          }
      }
    }
    Failed(_) | Closed => cell.val.state = Consumed
    ReaderDropped | Consumed => ()
  }
}
