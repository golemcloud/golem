// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1.

///|
pub(all) struct AgentBinary {
  bytes : Bytes
  mime_type : String?
} derive(Debug, Eq)

///|
pub struct AgentStream[T] {
  next_impl : async () -> T?
  cancel_impl : async (String) -> Unit
  mut terminal : Bool
  mut busy : Bool
  mut acquired : Bool
  mut input_owned : Bool
}

///|
pub fn[T] AgentStream::from_next(
  next : async () -> T?,
  on_cancel? : async (String) -> Unit = async fn(_) { () },
) -> AgentStream[T] {
  {
    next_impl: next,
    cancel_impl: on_cancel,
    terminal: false,
    busy: false,
    acquired: false,
    input_owned: false,
  }
}

///|
fn[T] AgentStream::acquire(self : AgentStream[T]) -> Unit raise {
  if self.acquired {
    raise BridgeError("stream-already-consumed: AgentStream is affine")
  }
  self.acquired = true
  self.input_owned = true
}

///|
pub async fn[T] AgentStream::next(self : AgentStream[T]) -> T? {
  if self.input_owned {
    raise BridgeError("stream-already-consumed: AgentStream ownership moved into an invocation")
  }
  if !self.acquired {
    self.acquired = true
  }
  self.pull_next()
}

///|
async fn[T] AgentStream::next_input(self : AgentStream[T]) -> T? {
  if !self.input_owned {
    raise BridgeError("AgentStream is not owned by an invocation")
  }
  self.pull_next()
}

///|
async fn[T] AgentStream::pull_next(self : AgentStream[T]) -> T? {
  if self.terminal {
    raise BridgeError("AgentStream is no longer readable")
  }
  if self.busy {
    raise BridgeError("AgentStream already has an outstanding next call")
  }
  self.busy = true
  let value = (self.next_impl)() catch {
    error => {
      self.busy = false
      self.terminal = true
      raise error
    }
  }
  self.busy = false
  if value is None {
    self.terminal = true
  }
  value
}

///|
pub async fn[T] AgentStream::cancel(self : AgentStream[T]) -> Unit {
  if self.input_owned {
    raise BridgeError("stream-already-consumed: AgentStream ownership moved into an invocation")
  }
  self.cancel_with_reason("cancelled")
}

///|
async fn[T] AgentStream::cancel_input(self : AgentStream[T]) -> Unit {
  if !self.input_owned {
    raise BridgeError("AgentStream is not owned by an invocation")
  }
  self.cancel_with_reason("cancelled")
}

///|
async fn[T] AgentStream::cancel_with_reason(
  self : AgentStream[T],
  reason : String,
) -> Unit {
  if !self.terminal {
    self.terminal = true
    (self.cancel_impl)(reason)
  }
}

///|
pub async fn[T] AgentStream::drop(self : AgentStream[T]) -> Unit {
  if self.input_owned {
    raise BridgeError("stream-already-consumed: AgentStream ownership moved into an invocation")
  }
  self.cancel_with_reason("consumer-drop")
}

///|
fn[T] remote_stream(
  next : async () -> T?,
  cancel : async (String) -> Unit,
) -> AgentStream[T] {
  AgentStream::from_next(next, on_cancel=cancel)
}
