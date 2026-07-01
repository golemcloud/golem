///|
pub(open) trait Waitable {
      update(Self, code~ : Int) -> Unit
      cancel(Self) -> Unit

      // when the waitable is dropped, this function is called to free resources
      drop(Self) -> Bool
      done(Self) -> Bool
      handle(Self) -> Int
}

///|
pub(all) enum SubtaskStatus {
      Starting(Int)
      Started(Int)
      Returned(Int)
      StartCancelled(Int)
      ReturnCancelled(Int)
} derive(Eq, Show)

///|
pub fn SubtaskStatus::decode(int : Int) -> SubtaskStatus {
      let handle = int >> 4
      match int & 0xf {
            0 => Starting(handle)
            1 => Started(handle)
            2 => Returned(handle)
            3 => StartCancelled(handle)
            4 => ReturnCancelled(handle)
            _ => panic()
      }
}

///|
pub fn SubtaskStatus::handle(self : Self) -> Int {
      match self {
            Starting(handle) => handle
            Started(handle) => handle
            Returned(handle) => handle
            StartCancelled(handle) => handle
            ReturnCancelled(handle) => handle
      }
}

///|
pub(all) enum Event {
      None
      Subtask
      StreamRead
      StreamWrite
      FutureRead
      FutureWrite
      TaskCancel
} derive(Eq, Show)

///|
pub fn Event::decode(int : Int) -> Event {
      match int {
            0 => None
            1 => Subtask
            2 => StreamRead
            3 => StreamWrite
            4 => FutureRead
            5 => FutureWrite
            6 => TaskCancel
            _ => panic()
      }
}

///|
pub fn Event::encode(self : Self) -> Int {
      match self {
            None => 0
            Subtask => 1
            StreamRead => 2
            StreamWrite => 3
            FutureRead => 4
            FutureWrite => 5
            TaskCancel => 6
      }
}

///|
pub(all) enum WaitableStatus {
      Completed(Int)
      Dropped(Int)
      Cancelled(Int)
      Blocking
} derive(Eq, Show)

///|
let waitable_status_block : Int = 0xffff_ffff

///|
pub fn WaitableStatus::decode(int : Int) -> WaitableStatus {
      if int == waitable_status_block {
            return Blocking
      }
      let amt = int >> 4
      match int & 0xf {
            0 => Completed(amt)
            1 => Dropped(amt)
            2 => Cancelled(amt)
            _ => panic()
      }
}

///|
pub fn WaitableStatus::count(int : Int) -> Int {
      int >> 4
}

///|
pub(all) enum CallbackCode {
      Exit
      Yield
      Wait(Int)
      Cancel(Int)
} derive(Eq, Show)

///|
pub fn CallbackCode::encode(self : Self) -> Int {
      match self {
            Exit => 0
            Yield => 1
            Wait(id) => 2 | (id << 4)
            Cancel(id) => 3 | (id << 4)
      }
}

///|
pub fn CallbackCode::decode(int : Int) -> CallbackCode {
      let id = int >> 4
      match int & 0xf {
            0 => Exit
            1 => Yield
            2 => Wait(id)
            3 => Cancel(id)
            _ => panic()
      }
}

///|
/// This function is empty, If you want to print debug info, you can hook it in your environment.
pub fn _async_debug(_msg : String) -> Unit {

}

// Component async primitives

///|
pub fn yield_blocking() -> Bool = "$root" "[yield]"

///|
pub fn backpressure_set() -> Int = "$root" "[backpressure-set]"

///|
pub fn subtask_cancel(id : Int) -> Int = "$root" "[subtask-cancel]"

///|
pub fn subtask_drop(id : Int) = "$root" "[subtask-drop]"

///|
pub fn context_set(task : Int) = "$root" "[context-set-0]"

///|
pub fn context_get() -> Int = "$root" "[context-get-0]"

///|
pub fn task_cancel() = "[export]$root" "[task-cancel]"

///|
pub fn waitable_set_new() -> Int = "$root" "[waitable-set-new]"

///|
pub fn waitable_set_drop(set : Int) = "$root" "[waitable-set-drop]"

///|
pub fn waitable_join(waitable : Int, set : Int) = "$root" "[waitable-join]"

///|
pub fn waitable_set_wait(set : Int, result_ptr : Int) -> Int = "$root" "[waitable-set-wait]"

///|
pub fn waitable_set_poll(set : Int, result_ptr : Int) -> Int = "$root" "[waitable-set-poll]"
