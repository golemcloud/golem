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
/// The baseline intrinsic cooperatively waits in the component model and
/// returns only after the subtask reaches a terminal state.
fn SubTask::cancel(self : SubTask) -> SubTaskState {
  SubTaskState::from(subtask_cancel(self.handle))
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
    0 => None
    1 => SubTask
    2 => StreamRead
    3 => StreamWrite
    4 => FutureRead
    5 => FutureWrite
    6 => TaskCancelled
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
#borrow(array)
extern "wasm" fn int_array2ptr(array : FixedArray[Int]) -> Int =
  #|(func (param i32) (result i32) local.get 0)

///|
struct WaitableSet(Int) derive(Eq, Hash)

///|
fn WaitableSet::new() -> WaitableSet {
  WaitableSet(waitable_set_new())
}

///|
fn WaitableSet::drop(self : Self) -> Unit {
  waitable_set_drop(self.0)
}

///|
fn WaitableSet::poll(self : Self) -> (Events, Int) {
  let payload : FixedArray[Int] = FixedArray::make(2, 0)
  let event = waitable_set_poll(self.0, int_array2ptr(payload))
  (Events::new(EventCode::from(event), payload[0], payload[1]), payload[0])
}

// #endregion

// #region Future

///|
priv enum FutureReadResult {
  Completed = 0
  Cancelled = 2
}

///|
fn FutureReadResult::from(int : Int) -> FutureReadResult {
  match int {
    0 => Completed
    2 => Cancelled
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
}

///|
fn CallbackCode::encode(self : Self) -> Int {
  match self {
    Completed => 0
    Yield => 1
    Wait(id) => 2 | (id.0 << 4)
  }
}

// #endregion

// #region Component async primitives

///|
fn subtask_cancel(id : Int) -> Int = "$root" "[subtask-cancel]"

///|
fn subtask_drop(id : Int) = "$root" "[subtask-drop]"

///|
fn tls_set(tls : Int) = "$root" "[context-set-0]"

///|
fn tls_get() -> Int = "$root" "[context-get-0]"

///|
fn task_cancel() = "[export]$root" "[task-cancel]"

///|
fn waitable_set_new() -> Int = "$root" "[waitable-set-new]"

///|
fn waitable_set_drop(set : Int) = "$root" "[waitable-set-drop]"

///|
/// This poll is deliberately non-cancellable. Component-task cancellation is
/// delivered by the stackless callback instead of being consumed here.
fn waitable_set_poll(set : Int, payload : Int) -> Int = "$root" "[waitable-set-poll]"

///|
fn task_wakeup_stream_new() -> Int64 = "$root" "[stream-new-unit]"

///|
fn task_wakeup_stream_read(handle : Int, ptr : Int, len : Int) -> Int = "$root" "[async-lower][stream-read-unit]"

///|
fn task_wakeup_stream_write(handle : Int, ptr : Int, len : Int) -> Int = "$root" "[async-lower][stream-write-unit]"

///|
fn task_wakeup_stream_cancel_read(handle : Int) -> Int = "$root" "[stream-cancel-read-unit]"

///|
fn task_wakeup_stream_drop_readable(handle : Int) = "$root" "[stream-drop-readable-unit]"

///|
fn task_wakeup_stream_drop_writable(handle : Int) = "$root" "[stream-drop-writable-unit]"

///|
fn waitable_join(waitable : Int, set : Int) = "$root" "[waitable-join]"

// #endregion
