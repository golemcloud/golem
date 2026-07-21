///|
pub struct Sink[X] {
  priv write : async (ArrayView[X]) -> Int
  priv close : async () -> Unit
  priv is_open : () -> Bool
  priv has_cleanup : () -> Bool
  priv cleanup : ((X) -> Unit)?
}

///|
let sink_write_window_size : Int = 64

///|
/// The write callback must either raise before consuming any value or return
/// the number consumed. It must not raise after partial consumption because
/// `ArrayView` cannot carry an ownership offset through an exception.
/// The close callback must be idempotent.
#internal(wit_bindgen, "generated binding code only")
pub fn[X] Sink::from_callbacks(
  write : async (ArrayView[X]) -> Int,
  close : async () -> Unit,
  is_open : () -> Bool,
  cleanup : ((X) -> Unit)?,
) -> Sink[X] {
  { write, close, is_open, has_cleanup: () => cleanup is Some(_), cleanup }
}

///|
/// Consumes up to one bounded window and returns the number of values consumed.
///
/// Consumption includes values accepted by the peer and values recursively
/// cleaned after the peer closes. Check `is_open()` to distinguish those cases.
pub async fn[X] Sink::write(self : Sink[X], data : ArrayView[X]) -> Int {
  if data.length() == 0 {
    return 0
  }
  let length = if data.length() < sink_write_window_size {
    data.length()
  } else {
    sink_write_window_size
  }
  // The callback may suspend, so it must not retain the caller's borrowed view.
  let buffer = FixedArray::makei(length, i => data[i])
  let written = (self.write)(buffer[:])
  guard written >= 0 && written <= length
  if !(self.is_open)() && written < length && (self.has_cleanup)() {
    self.cleanup_unaccepted(buffer[:], written)
    return length
  }
  written
}

///|
/// Writes one bounded window from an immutable byte view.
///
/// This is the byte-stream counterpart of `write(ArrayView[T])`. MoonBit keeps
/// `BytesView` and `ArrayView[Byte]` as distinct types, so byte strings need a
/// separate entry point until the language provides a shared read-only view.
pub async fn Sink::write_bytes(self : Sink[Byte], data : BytesView) -> Int {
  if data.length() == 0 {
    return 0
  }
  let length = if data.length() < sink_write_window_size {
    data.length()
  } else {
    sink_write_window_size
  }
  let buffer = FixedArray::makei(length, i => data[i])
  let written = (self.write)(buffer[:])
  guard written >= 0 && written <= length
  if !(self.is_open)() && written < length && (self.has_cleanup)() {
    self.cleanup_unaccepted(buffer[:], written)
    return length
  }
  written
}

///|
/// Returns whether the sink can still accept values from its peer.
pub fn[X] Sink::is_open(self : Sink[X]) -> Bool {
  (self.is_open)()
}

///|
fn[X] Sink::cleanup_unaccepted(
  self : Sink[X],
  data : ArrayView[X],
  offset : Int,
) -> Unit {
  match self.cleanup {
    Some(cleanup) =>
      for i in offset..<data.length() {
        cleanup(data[i])
      }
    None => ()
  }
}

///|
fn Sink::cleanup_unaccepted_bytes(
  self : Sink[Byte],
  data : BytesView,
  offset : Int,
) -> Unit {
  match self.cleanup {
    Some(cleanup) =>
      for i in offset..<data.length() {
        cleanup(data[i])
      }
    None => ()
  }
}

///|
/// Writes the entire slice to the sink unless the sink closes first.
///
/// Returns `true` if all items were written while the sink remained open and
/// `false` if the endpoint closed during the operation. When the sink has a
/// cleanup callback, every item was either accepted or cleaned and must not be
/// retried. Producers of explicitly managed resources must use such a sink.
pub async fn[X] Sink::write_all(self : Sink[X], data : ArrayView[X]) -> Bool {
  let mut offset = 0
  for ;; {
    if offset >= data.length() {
      return true
    }
    let written = self.write(data[offset:]) catch {
      err => {
        self.cleanup_unaccepted(data, offset)
        raise err
      }
    }
    offset = offset + written
    if !self.is_open() {
      self.cleanup_unaccepted(data, offset)
      return false
    }
    guard written > 0
  }
}

///|
/// Writes the entire byte view unless the sink closes first.
pub async fn Sink::write_all_bytes(self : Sink[Byte], data : BytesView) -> Bool {
  let mut offset = 0
  for ;; {
    if offset >= data.length() {
      return true
    }
    let written = self.write_bytes(data[offset:]) catch {
      err => {
        self.cleanup_unaccepted_bytes(data, offset)
        raise err
      }
    }
    offset = offset + written
    if !self.is_open() {
      self.cleanup_unaccepted_bytes(data, offset)
      return false
    }
    guard written > 0
  }
}

///|
/// Closes the writable end of the stream.
///
/// Closing is idempotent. A peer waiting for values observes the end of the
/// stream after any values already accepted by the sink have been read.
pub async fn[X] Sink::close(self : Sink[X]) -> Unit {
  (self.close)()
}

///|
priv struct LocalFutureState[X] {
  mut value : X?
  mut closed : Bool
  cleanup : ((X) -> Unit)?
}

///|
fn[X] local_future_close(
  state : Ref[LocalFutureState[X]],
  fallback_cleanup : ((X) -> Unit)?,
) -> Unit {
  if state.val.closed {
    return
  }
  match state.val.value {
    Some(value) => {
      state.val.value = None
      match state.val.cleanup {
        Some(cleanup) => cleanup(value)
        None =>
          match fallback_cleanup {
            Some(cleanup) => cleanup(value)
            None => ()
          }
      }
    }
    None => ()
  }
  state.val.closed = true
}

///|
fn[X] local_future_get(state : Ref[LocalFutureState[X]]) -> X raise {
  match state.val.value {
    Some(value) => {
      state.val.value = None
      state.val.closed = true
      value
    }
    None => raise Cancelled::Cancelled
  }
}

///|
priv struct StreamPipeReader[X] {
  max : Int
  mut value : FixedArray[X]?
  mut ended : Bool
  mut coro : Coroutine?
}

///|
priv struct StreamPipeWriter[X] {
  mut value : FixedArray[X]?
  mut accepted : Int
  mut coro : Coroutine?
}

///|
priv struct StreamPipe[X] {
  capacity : Int
  chunks : @deque.Deque[FixedArray[X]]
  mut buffered : Int
  mut writer_closed : Bool
  mut reader_dropped : Bool
  mut cleanup : ((X) -> Unit)?
  mut head : FixedArray[X]?
  mut head_pos : Int
  readers : @deque.Deque[StreamPipeReader[X]]
  writers : @deque.Deque[StreamPipeWriter[X]]
}

///|
fn[X] wake_stream_pipe_reader(
  reader : StreamPipeReader[X],
  value : FixedArray[X]?,
  ended : Bool,
) -> Unit {
  match reader.coro {
    Some(coro) => {
      reader.value = value
      reader.ended = ended
      reader.coro = None
      coro.wake()
    }
    None => ()
  }
}

///|
fn[X] wake_stream_pipe_writer(
  writer : StreamPipeWriter[X],
  accepted : Int,
) -> Unit {
  match writer.coro {
    Some(coro) => {
      writer.accepted = accepted
      writer.value = None
      writer.coro = None
      coro.wake()
    }
    None => writer.value = None
  }
}

///|
fn[X] take_stream_pipe_reader(
  pipe : Ref[StreamPipe[X]],
) -> StreamPipeReader[X]? {
  while pipe.val.readers.pop_front() is Some(reader) {
    if reader.coro is Some(_) {
      return Some(reader)
    }
  }
  None
}

///|
fn[X] take_stream_pipe_writer(
  pipe : Ref[StreamPipe[X]],
) -> StreamPipeWriter[X]? {
  while pipe.val.writers.pop_front() is Some(writer) {
    if writer.coro is Some(_) && writer.value is Some(_) {
      return Some(writer)
    }
  }
  None
}

///|
fn[X] fill_stream_pipe_from_writers(pipe : Ref[StreamPipe[X]]) -> Unit {
  if pipe.val.capacity <= 0 || pipe.val.writer_closed || pipe.val.reader_dropped {
    return
  }
  for ;; {
    let available = pipe.val.capacity - pipe.val.buffered
    if available <= 0 {
      return
    }
    guard take_stream_pipe_writer(pipe) is Some(writer) else { return }
    guard writer.value is Some(data) else { continue }
    let take = if available < data.length() { available } else { data.length() }
    let chunk = FixedArray::makei(take, i => data[i])
    pipe.val.chunks.push_back(chunk)
    pipe.val.buffered = pipe.val.buffered + take
    wake_stream_pipe_writer(writer, take)
  }
}

///|
fn[X] stream_pipe_drop_reader(pipe : Ref[StreamPipe[X]]) -> Unit {
  stream_pipe_drop_reader_with_cleanup(pipe, pipe.val.cleanup)
}

///|
fn[X] stream_pipe_drop_reader_with_cleanup(
  pipe : Ref[StreamPipe[X]],
  cleanup : ((X) -> Unit)?,
) -> Unit {
  if pipe.val.reader_dropped {
    return
  }
  match cleanup {
    Some(cleanup) => {
      match pipe.val.head {
        Some(head) =>
          for i in pipe.val.head_pos..<head.length() {
            cleanup(head[i])
          }
        None => ()
      }
      while pipe.val.chunks.pop_front() is Some(chunk) {
        for value in chunk {
          cleanup(value)
        }
      }
    }
    None => ()
  }
  pipe.val.head = None
  pipe.val.head_pos = 0
  pipe.val.chunks.clear()
  pipe.val.buffered = 0
  pipe.val.reader_dropped = true
  while pipe.val.readers.pop_front() is Some(reader) {
    wake_stream_pipe_reader(reader, None, true)
  }
  while pipe.val.writers.pop_front() is Some(writer) {
    let consumed = match (cleanup, writer.value) {
      (Some(cleanup), Some(data)) => {
        for value in data {
          cleanup(value)
        }
        data.length()
      }
      _ => 0
    }
    wake_stream_pipe_writer(writer, consumed)
  }
}

///|
fn[X] stream_pipe_reject_reader(
  pipe : Ref[StreamPipe[X]],
  fallback : (X) -> Unit,
) -> Unit {
  if pipe.val.cleanup is None {
    pipe.val.cleanup = Some(fallback)
  }
  stream_pipe_drop_reader(pipe)
}

///|
fn[X] stream_pipe_close_writer(pipe : Ref[StreamPipe[X]]) -> Unit {
  if pipe.val.writer_closed {
    return
  }
  pipe.val.writer_closed = true
  while pipe.val.writers.pop_front() is Some(writer) {
    wake_stream_pipe_writer(writer, 0)
  }
  if pipe.val.buffered == 0 {
    while pipe.val.readers.pop_front() is Some(reader) {
      wake_stream_pipe_reader(reader, None, true)
    }
  }
}

///|
async fn[X] stream_pipe_write(
  pipe : Ref[StreamPipe[X]],
  data : ArrayView[X],
) -> Int {
  if data.length() == 0 || pipe.val.writer_closed || pipe.val.reader_dropped {
    return 0
  }
  match take_stream_pipe_reader(pipe) {
    Some(reader) => {
      let take = if reader.max < data.length() {
        reader.max
      } else {
        data.length()
      }
      let chunk = FixedArray::makei(take, i => data[i])
      wake_stream_pipe_reader(reader, Some(chunk), false)
      return take
    }
    None => ()
  }
  if pipe.val.capacity > 0 && pipe.val.buffered < pipe.val.capacity {
    let available = pipe.val.capacity - pipe.val.buffered
    let take = if available < data.length() { available } else { data.length() }
    let chunk = FixedArray::makei(take, i => data[i])
    pipe.val.chunks.push_back(chunk)
    pipe.val.buffered = pipe.val.buffered + take
    return take
  }
  let writer = StreamPipeWriter::{
    value: Some(FixedArray::makei(data.length(), i => data[i])),
    accepted: 0,
    coro: Some(current_coroutine()),
  }
  pipe.val.writers.push_back(writer)
  suspend() catch {
    _ if writer.coro is None => return writer.accepted
    err => {
      writer.coro = None
      writer.value = None
      raise err
    }
  }
  writer.value = None
  writer.accepted
}

///|
async fn[X] stream_pipe_read(
  pipe : Ref[StreamPipe[X]],
  count : Int,
) -> FixedArray[X]? {
  if count <= 0 {
    return Some([])
  }
  for ;; {
    if pipe.val.reader_dropped {
      return None
    }
    match pipe.val.head {
      Some(head) => {
        let available = head.length() - pipe.val.head_pos
        let take = if count < available { count } else { available }
        let result = FixedArray::makei(take, i => head[pipe.val.head_pos + i])
        pipe.val.head_pos = pipe.val.head_pos + take
        pipe.val.buffered = pipe.val.buffered - take
        if pipe.val.head_pos >= head.length() {
          pipe.val.head = None
          pipe.val.head_pos = 0
        }
        fill_stream_pipe_from_writers(pipe)
        return Some(result)
      }
      None => ()
    }
    if pipe.val.chunks.pop_front() is Some(chunk) {
      pipe.val.head = Some(chunk)
      pipe.val.head_pos = 0
      continue
    }
    match take_stream_pipe_writer(pipe) {
      Some(writer) => {
        guard writer.value is Some(data) else { continue }
        let take = if count < data.length() { count } else { data.length() }
        let result = FixedArray::makei(take, i => data[i])
        wake_stream_pipe_writer(writer, take)
        return Some(result)
      }
      None => ()
    }
    if pipe.val.writer_closed {
      return None
    }
    let reader = StreamPipeReader::{
      max: count,
      value: None,
      ended: false,
      coro: Some(current_coroutine()),
    }
    pipe.val.readers.push_back(reader)
    suspend() catch {
      err => {
        reader.coro = None
        match reader.value {
          Some(value) => return Some(value)
          None if reader.ended => return None
          None => raise err
        }
      }
    }
    match reader.value {
      Some(value) => return Some(value)
      None if reader.ended => return None
      None => ()
    }
  }
}

///|
priv struct FutureSource[X] {
  get : async () -> X
  close : async () -> Unit
  close_sync : () -> Unit
}

///|
priv struct FutureProducerState[X] {
  mut producer : (async () -> X)?
  mut on_unstarted_drop : (() -> Unit)?
}

///|
priv enum FutureState[X] {
  Source(FutureSource[X])
  Local(Ref[LocalFutureState[X]])
  Pending(Ref[FutureProducerState[X]])
  Promised(Ref[PromiseCell[X]])
}

///|
pub struct Future[X] {
  priv inner : FutureState[X]
}

///|
pub fn[X] Future::ready_with_cleanup(
  value : X,
  cleanup : (X) -> Unit,
) -> Future[X] {
  let state : Ref[LocalFutureState[X]] = {
    val: { value: Some(value), closed: false, cleanup: Some(cleanup) },
  }
  { inner: Local(state) }
}

///|
pub fn[X] Future::ready(value : X) -> Future[X] {
  let state : Ref[LocalFutureState[X]] = {
    val: { value: Some(value), closed: false, cleanup: None },
  }
  { inner: Local(state) }
}

///|
/// Creates a lazy future from an async producer.
///
/// `on_unstarted_drop`, when present, releases resources captured by a producer
/// that is discarded before its first `get`. Once the producer starts, it owns
/// its runtime cleanup and this callback is never invoked.
pub fn[X] Future::from(
  producer : async () -> X,
  on_unstarted_drop? : () -> Unit,
) -> Future[X] {
  { inner: Pending({ val: { producer: Some(producer), on_unstarted_drop } }) }
}

///|
#internal(wit_bindgen, "generated binding code only")
pub fn[X] Future::from_source(
  get : async () -> X,
  close : async () -> Unit,
  close_sync : () -> Unit,
) -> Future[X] {
  { inner: Source({ get, close, close_sync }) }
}

///|
/// Waits for and consumes the future's single value.
///
/// A `Future` is a one-shot value. Calling `get` again, or calling it after the
/// readable end has been dropped, fails instead of replaying the value.
pub async fn[X] Future::get(self : Future[X]) -> X {
  match self.inner {
    Source(source) => (source.get)()
    Local(state) => local_future_get(state)
    Pending(state) =>
      match state.val.producer {
        Some(producer) => {
          state.val.producer = None
          state.val.on_unstarted_drop = None
          producer()
        }
        None => raise Cancelled::Cancelled
      }
    Promised(cell) => promised_future_get(cell)
  }
}

///|
/// Releases the readable end without consuming its value.
///
/// If the value is already locally available, its registered cleanup callback
/// runs. An unstarted lazy producer is discarded and runs
/// `on_unstarted_drop`, when provided.
pub async fn[X] Future::drop(self : Future[X]) -> Unit noraise {
  match self.inner {
    Source(source) => (source.close)() catch { _ => () }
    Local(state) => local_future_close(state, None)
    Pending(state) => {
      let cleanup = state.val.on_unstarted_drop
      state.val.producer = None
      state.val.on_unstarted_drop = None
      match cleanup {
        Some(cleanup) => cleanup()
        None => ()
      }
    }
    Promised(cell) => promised_future_drop(cell, None)
  }
}

///|
#internal(wit_bindgen, "generated binding code only")
pub fn[X] Future::drop_sync(self : Future[X]) -> Unit {
  match self.inner {
    Source(source) => (source.close_sync)()
    Local(state) => local_future_close(state, None)
    Pending(state) => {
      let cleanup = state.val.on_unstarted_drop
      state.val.producer = None
      state.val.on_unstarted_drop = None
      match cleanup {
        Some(cleanup) => cleanup()
        None => ()
      }
    }
    Promised(cell) => promised_future_drop(cell, None)
  }
}

///|
/// Rejects a future that was prepared for a component-model transfer but was
/// never transferred. A locally available value uses its registered cleanup,
/// falling back to the generated canonical-ABI cleanup. An unstarted producer
/// is discarded without running it.
#internal(wit_bindgen, "generated binding code only")
pub async fn[X] Future::reject(
  self : Future[X],
  cleanup : (X) -> Unit,
) -> Unit noraise {
  match self.inner {
    Source(source) => (source.close)() catch { _ => () }
    Local(state) => local_future_close(state, Some(cleanup))
    Pending(state) => {
      let cleanup = state.val.on_unstarted_drop
      state.val.producer = None
      state.val.on_unstarted_drop = None
      match cleanup {
        Some(cleanup) => cleanup()
        None => ()
      }
    }
    Promised(cell) => promised_future_drop(cell, Some(cleanup))
  }
}

///|
priv struct StreamProducerState[X] {
  mut producer : (async (Sink[X]) -> Unit)?
  mut cleanup : ((X) -> Unit)?
  mut on_unstarted_drop : (() -> Unit)?
  mut pipe : Ref[StreamPipe[X]]?
}

///|
priv struct StreamSource[X] {
  read : async (Int) -> FixedArray[X]?
  close : async () -> Unit
  close_sync : () -> Unit
}

///|
priv enum StreamState[X] {
  Source(StreamSource[X])
  Local(Ref[StreamPipe[X]])
  Outgoing(Ref[StreamProducerState[X]])
}

///|
pub struct Stream[X] {
  priv inner : StreamState[X]
}

///|
fn[X] new_stream_pipe(
  capacity : Int,
  cleanup : ((X) -> Unit)?,
) -> Ref[StreamPipe[X]] {
  if capacity < 0 {
    abort("stream capacity must not be negative")
  }
  {
    val: {
      capacity,
      chunks: Deque([]),
      buffered: 0,
      writer_closed: false,
      reader_dropped: false,
      cleanup,
      head: None,
      head_pos: 0,
      readers: Deque([]),
      writers: Deque([]),
    },
  }
}

///|
fn[X] stream_pipe_sink(pipe : Ref[StreamPipe[X]]) -> Sink[X] {
  {
    write: (data : ArrayView[X]) => stream_pipe_write(pipe, data),
    close: () => stream_pipe_close_writer(pipe),
    is_open: () => !pipe.val.writer_closed && !pipe.val.reader_dropped,
    has_cleanup: () => pipe.val.cleanup is Some(_),
    cleanup: Some(fn(value) {
      match pipe.val.cleanup {
        Some(cleanup) => cleanup(value)
        None => ()
      }
    }),
  }
}

///|
fn[X] new_local_stream(
  capacity : Int,
  cleanup : ((X) -> Unit)?,
) -> (Stream[X], Sink[X]) {
  let pipe = new_stream_pipe(capacity, cleanup)
  let stream = Stream::{ inner: Local(pipe) }
  let sink = stream_pipe_sink(pipe)
  (stream, sink)
}

///|
fn spawn_stream_producer(task : async () -> Unit) -> Unit {
  let current = current_coroutine()
  match current.spawner {
    Some(spawner) => spawner(task)
    None => ignore(spawn(task))
  }
}

///|
fn[X] materialize_stream_producer(
  state : Ref[StreamProducerState[X]],
) -> Ref[StreamPipe[X]] {
  match state.val.pipe {
    Some(pipe) => pipe
    None => {
      let pipe = new_stream_pipe(0, state.val.cleanup)
      state.val.cleanup = None
      state.val.pipe = Some(pipe)
      match state.val.producer {
        Some(producer) => {
          state.val.producer = None
          state.val.on_unstarted_drop = None
          let sink = stream_pipe_sink(pipe)
          spawn_stream_producer(async fn() {
            producer(sink) catch {
              _ => {
                sink.close() catch {
                  _ => ()
                }
                return
              }
            }
            sink.close()
          })
        }
        None => stream_pipe_close_writer(pipe)
      }
      pipe
    }
  }
}

///|
pub fn[X] Stream::new(capacity? : Int = 0) -> (Stream[X], Sink[X]) {
  new_local_stream(capacity, None)
}

///|
/// Creates a local stream whose unread values are cleaned up when its readable
/// end is dropped.
pub fn[X] Stream::new_with_cleanup(
  cleanup : (X) -> Unit,
  capacity? : Int = 0,
) -> (Stream[X], Sink[X]) {
  new_local_stream(capacity, Some(cleanup))
}

///|
/// Creates a lazy stream from an async producer.
///
/// `cleanup`, when present, releases buffered or pending values if the readable
/// end is dropped after the producer starts. Producers of explicitly managed
/// resources should provide it.
///
/// `on_unstarted_drop`, when present, releases resources captured by a producer
/// that is discarded before its first read or before a prepared component
/// transfer commits. Once the producer starts, it owns its runtime cleanup and
/// this callback is never invoked.
pub fn[X] Stream::produce(
  producer : async (Sink[X]) -> Unit,
  cleanup? : (X) -> Unit,
  on_unstarted_drop? : () -> Unit,
) -> Stream[X] {
  {
    inner: Outgoing({
      val: { producer: Some(producer), cleanup, on_unstarted_drop, pipe: None },
    }),
  }
}

///|
#internal(wit_bindgen, "generated binding code only")
pub fn[X] Stream::from_source(
  read : async (Int) -> FixedArray[X]?,
  close : async () -> Unit,
  close_sync : () -> Unit,
) -> Stream[X] {
  { inner: Source({ read, close, close_sync }) }
}

///|
/// Reads at most `count` values from the stream.
///
/// `Some(values)` contains the next owned chunk and may be empty when the peer
/// reports no progress. `None` means the writable end has closed and no
/// buffered values remain. A non-positive count returns an empty chunk without
/// waiting.
pub async fn[X] Stream::read(self : Stream[X], count : Int) -> FixedArray[X]? {
  match self.inner {
    Source(source) => (source.read)(count)
    Local(pipe) => stream_pipe_read(pipe, count)
    Outgoing(state) =>
      stream_pipe_read(materialize_stream_producer(state), count)
  }
}

///|
/// Releases the readable end and discards unread values.
///
/// Registered cleanup callbacks run for locally buffered values. An unstarted
/// lazy producer is discarded and runs `on_unstarted_drop`, when provided.
pub async fn[X] Stream::drop(self : Stream[X]) -> Unit noraise {
  match self.inner {
    Source(source) => (source.close)() catch { _ => () }
    Local(pipe) => stream_pipe_drop_reader(pipe)
    Outgoing(state) =>
      match state.val.pipe {
        Some(pipe) => stream_pipe_drop_reader(pipe)
        None => {
          let cleanup = state.val.on_unstarted_drop
          state.val.producer = None
          state.val.cleanup = None
          state.val.on_unstarted_drop = None
          match cleanup {
            Some(cleanup) => cleanup()
            None => ()
          }
        }
      }
  }
}

///|
#internal(wit_bindgen, "generated binding code only")
pub fn[X] Stream::drop_sync(self : Stream[X]) -> Unit {
  match self.inner {
    Source(source) => (source.close_sync)()
    Local(pipe) => stream_pipe_drop_reader(pipe)
    Outgoing(state) =>
      match state.val.pipe {
        Some(pipe) => stream_pipe_drop_reader(pipe)
        None => {
          let cleanup = state.val.on_unstarted_drop
          state.val.producer = None
          state.val.cleanup = None
          state.val.on_unstarted_drop = None
          match cleanup {
            Some(cleanup) => cleanup()
            None => ()
          }
        }
      }
  }
}

///|
#internal(wit_bindgen, "generated binding code only")
pub fn[X] Stream::take_producer(self : Stream[X]) -> (async (Sink[X]) -> Unit)? {
  match self.inner {
    Outgoing(state) =>
      match state.val.pipe {
        Some(_) => None
        None => {
          let producer = state.val.producer
          state.val.producer = None
          state.val.cleanup = None
          state.val.on_unstarted_drop = None
          producer
        }
      }
    Source(_) | Local(_) => None
  }
}

///|
/// Rejects a stream that was prepared for a component-model transfer but was
/// never transferred. Component sources are closed immediately and buffered
/// local values are cleaned synchronously. An unstarted producer is discarded
/// without running it; its optional `on_unstarted_drop` callback owns cleanup of
/// captured resources.
#internal(wit_bindgen, "generated binding code only")
pub async fn[X] Stream::reject(
  self : Stream[X],
  cleanup : (X) -> Unit,
) -> Unit noraise {
  match self.inner {
    Source(source) => (source.close)() catch { _ => () }
    Local(pipe) => stream_pipe_reject_reader(pipe, cleanup)
    Outgoing(state) =>
      match state.val.pipe {
        Some(pipe) => stream_pipe_reject_reader(pipe, cleanup)
        None => {
          let cleanup = state.val.on_unstarted_drop
          state.val.producer = None
          state.val.cleanup = None
          state.val.on_unstarted_drop = None
          match cleanup {
            Some(cleanup) => cleanup()
            None => ()
          }
        }
      }
  }
}
