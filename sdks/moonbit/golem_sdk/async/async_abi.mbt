// #region subtask

///|
priv struct SubTask {
      handle : Int
      state : SubTaskState
}

///|
priv enum SubTaskState {
      Starting = 0
      Started = 1
      Returned = 2
      Cancelled_before_started = 3
      Cancelled_before_returned = 4
}

///|
fn SubTaskState::from(int : Int) -> SubTaskState {
      match int {
            0 => Starting
            1 => Started
            2 => Returned
            3 => Cancelled_before_started
            4 => Cancelled_before_returned
            _ => panic()
      }
}

///|
fn SubTask::from(code : Int) -> SubTask {
      { handle: code >> 4, state: SubTaskState::from(code & 0xf) }
}

///|
/// None : the subtask is blocked
fn SubTask::cancel(self : SubTask) -> SubTaskState? {
      let result = subtask_cancel(self.handle)
      if result == -1 {
            None
      } else {
            Some(SubTaskState::from(subtask_cancel(self.handle)))
      }
}

// #endregion

// #region events

///|
priv enum EventCode {
      None = 0
      SubTask = 1
      StreamRead = 2
      StreamWrite = 3
      FutureRead = 4
      FutureWrite = 5
      TaskCancelled = 6
}

///|
fn EventCode::from(int : Int) -> EventCode {
      match int {
            0 => EventCode::None
            1 => EventCode::SubTask
            2 => EventCode::StreamRead
            3 => EventCode::StreamWrite
            4 => EventCode::FutureRead
            5 => EventCode::FutureWrite
            6 => EventCode::TaskCancelled
            _ => panic()
      }
}

///|
priv enum Events {
      None
      Subtask(Int, SubTaskState)
      StreamRead(Int, StreamResult)
      StreamWrite(Int, StreamResult)
      FutureRead(Int, FutureReadResult)
      FutureWrite(Int, FutureWriteResult)
      TaskCancelled
}

///|
fn Events::new(code : EventCode, i : Int, j : Int) -> Events {
      match code {
            None => None
            SubTask => Subtask(i, SubTaskState::from(j))
            StreamRead => StreamRead(i, StreamResult::from(j))
            StreamWrite => StreamWrite(i, StreamResult::from(j))
            FutureRead => FutureRead(i, FutureReadResult::from(j))
            FutureWrite => FutureWrite(i, FutureWriteResult::from(j))
            TaskCancelled => TaskCancelled
      }
}

// #endregion

// #region waitable set

///|
struct WaitableSet(Int) derive(Eq, Show, Hash)

///|
fn WaitableSet::new() -> WaitableSet {
      WaitableSet(waitable_set_new())
}

///|
fn WaitableSet::drop(self : Self) -> Unit {
      waitable_set_drop(self.0)
}

// #endregion

// #region Future

///|
priv enum FutureReadResult {
      Completed = 0
      Cancelled = 1
}

///|
fn FutureReadResult::from(int : Int) -> FutureReadResult {
      match int {
            0 => Completed
            1 => Cancelled
            _ => panic()
      }
}

///|
priv enum FutureWriteResult {
      Completed = 0
      Dropped = 1
      Cancelled = 2
}

///|
fn FutureWriteResult::from(int : Int) -> FutureWriteResult {
      match int {
            0 => Completed
            1 => Dropped
            2 => Cancelled
            _ => panic()
      }
}

// #endregion

// #region Stream

///|
priv struct StreamResult {
      progress : Int
      copy_result : CopyResult
}

///|
fn StreamResult::from(int : Int) -> StreamResult {
      let progress = int >> 4
      let copy_result = CopyResult::from(int & 0xf)
      { progress, copy_result }
}

///|
priv enum CopyResult {
      Completed = 0
      Dropped = 1
      Cancelled = 2
}

///|
fn CopyResult::from(int : Int) -> CopyResult {
      match int {
            0 => Completed
            1 => Dropped
            2 => Cancelled
            _ => panic()
      }
}

// #endregion

// #region callback code

///|
/// Code to let the runtime know what to do in the callback
priv enum CallbackCode {
      Completed
      Yield
      Wait(WaitableSet)
      Poll(WaitableSet)
}

///|
fn CallbackCode::encode(self : Self) -> Int {
      match self {
            Completed => 0
            Yield => 1
            Wait(id) => 2 | (id.0 << 4)
            Poll(id) => 3 | (id.0 << 4)
      }
}

///|
fn CallbackCode::decode(int : Int) -> CallbackCode {
      let id = int >> 4
      match int & 0xf {
            0 => Completed
            1 => Yield
            2 => Wait(id)
            3 => Poll(id)
            _ => panic()
      }
}

// #endregion

// #region Component async primitives

///|
/// Return whether is cancelled.
/// Use for non-callback implementation.
fn _yield() -> Bool = "$root" "[cancellable][yield]"

///|
fn _backpressure_inc() = "$root" "[backpressure-inc]"

///|
fn _backpressure_dec() = "$root" "[backpressure-dec]"

///|
fn subtask_cancel(id : Int) -> Int = "$root" "[subtask-cancel]"

///|
fn subtask_drop(id : Int) = "$root" "[subtask-drop]"

///|
pub fn context_set(task : Int) = "$root" "[context-set-0]"

///|
pub fn context_get() -> Int = "$root" "[context-get-0]"

///|
fn tls_set(tls : Int) = "$root" "[context-set-0]"

///|
fn tls_get() -> Int = "$root" "[context-get-0]"

///|
pub fn task_cancel() = "[export]$root" "[task-cancel]"

///|
fn waitable_set_new() -> Int = "$root" "[waitable-set-new]"

///|
fn waitable_set_drop(set : Int) = "$root" "[waitable-set-drop]"

///|
fn waitable_join(waitable : Int, set : Int) = "$root" "[waitable-join]"

// #endregion
