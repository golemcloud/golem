///|
pub struct Subtask {
      handle : Int
      mut code : Int?
      mut dropped : Bool
}

///|
pub fn Subtask::from_handle(handle : Int, code? : Int) -> Subtask {
      { handle, code, dropped: false }
}

///|
pub impl Waitable for Subtask with update(self, code~ : Int) -> Unit {
      self.code = Some(code)
}

///|
pub impl Eq for Subtask with equal(self, other) -> Bool {
      self.handle == other.handle
}

///|
pub impl Waitable for Subtask with handle(self) -> Int {
      self.handle
}

///|
pub impl Waitable for Subtask with cancel(self) -> Unit {
      if self.code is Some(code) && CallbackCode::decode(code) is Cancel(_) {
            return
      }
      self.code = Some(subtask_cancel(self.handle))
}

///|
pub impl Waitable for Subtask with drop(self) -> Bool {
      _async_debug("subtask-drop(\{self.handle})")
      if self.done() || self.dropped {
            return false
      }
      subtask_drop(self.handle)
      self.dropped = true
      true
}

///|
pub impl Waitable for Subtask with done(self) -> Bool {
      guard self.code is Some(code) else { return false }
      match SubtaskStatus::decode(code) {
            StartCancelled(_) => true
            Returned(_) => true
            Started(_) => false
            Starting(_) => false
            ReturnCancelled(_) => true
      }
}
