///|
pub struct FutureVTable[T] {
      new : () -> UInt64
      read : (Int, Int) -> Int
      write : (Int, Int) -> Int
      cancel_read : (Int) -> Int
      cancel_write : (Int) -> Int
      drop_readable : (Int) -> Unit
      drop_writable : (Int) -> Unit
      malloc : (Int) -> Int
      free : (Int) -> Unit
      lift : (Int) -> T
      lower : (T, Int) -> Unit
}

///|
pub fn[T] FutureVTable::new(
new : () -> UInt64,
read : (Int, Int) -> Int,
write : (Int, Int) -> Int,
cancel_read : (Int) -> Int,
cancel_write : (Int) -> Int,
drop_readable : (Int) -> Unit,
drop_writable : (Int) -> Unit,
malloc : (Int) -> Int,
free : (Int) -> Unit,
lift : (Int) -> T,
lower : (T, Int) -> Unit,
) -> FutureVTable[T] {
      {
            new,
            read,
            write,
            cancel_read,
            cancel_write,
            drop_readable,
            drop_writable,
            malloc,
            free,
            lift,
            lower,
      }
}

///|
pub fn[T] new_future(
vtable : FutureVTable[T],
) -> (FutureReader[T], FutureWriter[T]) {
      let handle = (vtable.new)()
      let left_handle = handle.to_int()
      let right_handle = (handle >> 32).to_int()
      (
      FutureReader::new(left_handle, vtable),
      FutureWriter::new(right_handle, vtable),
      )
}

///|
pub struct FutureReader[T] {
      handle : Int
      vtable : FutureVTable[T]
      mut code : Int?
      mut dropped : Bool
      memory_refs : Array[Int]
}

///|
pub fn[T] FutureReader::new(
handle : Int,
vtable : FutureVTable[T],
) -> FutureReader[T] {
      { handle, vtable, code: None, memory_refs: [], dropped: false }
}

///|
pub impl[T] Waitable for FutureReader[T] with update(self, code~ : Int) -> Unit {
      self.code = Some(code)
}

///|
pub impl[T] Eq for FutureReader[T] with equal(self, other) -> Bool {
      self.handle == other.handle
}

///|
pub impl[T] Waitable for FutureReader[T] with handle(self) -> Int {
      self.handle
}

///|
pub impl[T] Waitable for FutureReader[T] with cancel(self) -> Unit {
      if self.code is Some(code) && WaitableStatus::decode(code) is Cancelled(_) {
            return
      }
      self.code = Some((self.vtable.cancel_read)(self.handle))
}

///|
pub impl[T] Waitable for FutureReader[T] with drop(self) -> Bool {
      _async_debug("stream-reader-drop(\{self.handle})")
      if self.dropped {
            return false
      }
      (self.vtable.drop_readable)(self.handle)
      self.dropped = true
      for ptr in self.memory_refs {
            self.free(ptr)
      }
      true
}

///|
pub impl[T] Waitable for FutureReader[T] with done(self) -> Bool {
      match self.code {
            Some(c) =>
            match WaitableStatus::decode(c) {
                  Completed(_) | Dropped(_) | Cancelled(_) => true
                  Blocking => false
            }
            None => false
      }
}

///|
pub fn[T] FutureReader::malloc(self : FutureReader[T]) -> Int {
      let ptr = (self.vtable.malloc)(1)
      ptr
}

///|
pub fn[T] FutureReader::free(self : FutureReader[T], ptr : Int) -> Unit {
      (self.vtable.free)(ptr)
}

///|
pub fn[T] FutureReader::lift(self : FutureReader[T], ptr : Int) -> T {
      let res = (self.vtable.lift)(ptr)
      res
}

///|
pub fn[T] FutureReader::lower_read(self : FutureReader[T], ptr : Int) -> Int {
      (self.vtable.read)(self.handle, ptr)
}

///|
pub async fn[T] FutureReader::read(self : FutureReader[T]) -> T {
      let buf_ptr = self.malloc()
      self.memory_refs.push(buf_ptr)
      self.code = Some(self.lower_read(buf_ptr))
      _async_debug("future-read(\{self.handle}) -> \{self.code.unwrap()}")
      // register this waitable to the current task
      let task = current_task()
      task.add_waitable(self, current_coroutine())
      defer task.remove_waitable(self)

      // wait until ready
      for {
            let status = WaitableStatus::decode(self.code.unwrap())
            match status {
                  Cancelled(_) | Dropped(_) => raise Cancelled::Cancelled
                  Completed(_) => break
                  Blocking => suspend()
            }
      }
      // when receive event, continue this coroutine
      let value = self.lift(buf_ptr)
      return value
}

///|
pub struct FutureWriter[T] {
      handle : Int
      vtable : FutureVTable[T]
      mut code : Int?
      mut dropped : Bool
      memory_refs : Array[Int]
}

///|
pub fn[T] FutureWriter::new(
handle : Int,
vtable : FutureVTable[T],
) -> FutureWriter[T] {
      { handle, vtable, code: None, memory_refs: [], dropped: false }
}

///|
pub impl[T] Waitable for FutureWriter[T] with update(self, code~ : Int) -> Unit {
      self.code = Some(code)
}

///|
pub impl[T] Eq for FutureWriter[T] with equal(self, other) -> Bool {
      self.handle == other.handle
}

///|
pub impl[T] Waitable for FutureWriter[T] with handle(self) -> Int {
      self.handle
}

///|
pub impl[T] Waitable for FutureWriter[T] with cancel(self) -> Unit {
      if self.code is Some(code) && WaitableStatus::decode(code) is Cancelled(_) {
            return
      }
      self.code = Some((self.vtable.cancel_write)(self.handle))
}

///|
pub impl[T] Waitable for FutureWriter[T] with drop(self) -> Bool {
      _async_debug("stream-writer-drop(\{self.handle})")
      if self.dropped {
            return false
      }
      (self.vtable.drop_writable)(self.handle)
      self.dropped = true
      for ptr in self.memory_refs {
            self.free(ptr)
      }
      true
}

///|
pub impl[T] Waitable for FutureWriter[T] with done(self) -> Bool {
      match self.code {
            Some(c) =>
            match WaitableStatus::decode(c) {
                  Completed(_) | Dropped(_) | Cancelled(_) => true
                  Blocking => false
            }
            None => false
      }
}

///|
pub fn[T] FutureWriter::malloc(self : FutureWriter[T]) -> Int {
      (self.vtable.malloc)(1)
}

///|
pub fn[T] FutureWriter::free(self : FutureWriter[T], ptr : Int) -> Unit {
      (self.vtable.free)(ptr)
}

///|
pub fn[T] FutureWriter::lower(
self : FutureWriter[T],
value : T,
ptr : Int,
) -> Unit {
      (self.vtable.lower)(value, ptr)
}

///|
pub fn[T] FutureWriter::lower_write(self : FutureWriter[T], ptr : Int) -> Int {
      (self.vtable.write)(self.handle, ptr)
}

///|
pub async fn[T] FutureWriter::write(self : FutureWriter[T], value : T) -> Unit {
      // register this waitable to the current task
      let task = current_task()
      task.add_waitable(self, current_coroutine())
      defer task.remove_waitable(self)
      let buf_ptr = self.malloc()
      self.memory_refs.push(buf_ptr)
      self.lower(value, buf_ptr)
      self.code = Some(self.lower_write(buf_ptr))
      defer self.free(buf_ptr)

      // wait until ready
      for {
            let status = WaitableStatus::decode(self.code.unwrap())
            match status {
                  Cancelled(_) | Dropped(_) => raise Cancelled::Cancelled
                  Completed(_) => break
                  Blocking => suspend()
            }
      }
      // when receive event, continue this coroutine
      return
}

///|
pub suberror StreamCancelled (Int, Cancelled) derive(Show)

///|
pub struct StreamVTable[T] {
      new : () -> UInt64
      read : (Int, Int, Int) -> Int
      write : (Int, Int, Int) -> Int
      cancel_read : (Int) -> Int
      cancel_write : (Int) -> Int
      drop_readable : (Int) -> Unit
      drop_writable : (Int) -> Unit
      malloc : (Int) -> Int
      free : (Int) -> Unit
      lift : (Int, Int) -> FixedArray[T]
      lower : (FixedArray[T]) -> Int
}

///|
pub fn[T] StreamVTable::new(
new : () -> UInt64,
read : (Int, Int, Int) -> Int,
write : (Int, Int, Int) -> Int,
cancel_read : (Int) -> Int,
cancel_write : (Int) -> Int,
drop_readable : (Int) -> Unit,
drop_writable : (Int) -> Unit,
malloc : (Int) -> Int,
free : (Int) -> Unit,
lift : (Int, Int) -> FixedArray[T],
lower : (FixedArray[T]) -> Int,
) -> StreamVTable[T] {
      {
            new,
            read,
            write,
            cancel_read,
            cancel_write,
            drop_readable,
            drop_writable,
            malloc,
            free,
            lift,
            lower,
      }
}

///|
pub fn[T] new_stream(
vtable : StreamVTable[T],
) -> (StreamReader[T], StreamWriter[T]) {
      let handle = (vtable.new)()
      let left_handle = handle.to_int()
      let right_handle = (handle >> 32).to_int()
      (
      StreamReader::new(left_handle, vtable),
      StreamWriter::new(right_handle, vtable),
      )
}

///|
pub struct StreamReader[T] {
      handle : Int
      vtable : StreamVTable[T]
      mut code : Int?
      mut dropped : Bool
      memory_refs : Array[Int]
}

///|
pub impl[T] Waitable for StreamReader[T] with update(self, code~ : Int) -> Unit {
      self.code = Some(code)
}

///|
pub impl[T] Eq for StreamReader[T] with equal(self, other) -> Bool {
      self.handle == other.handle
}

///|
pub impl[T] Waitable for StreamReader[T] with handle(self) -> Int {
      self.handle
}

///|
pub impl[T] Waitable for StreamReader[T] with cancel(self) -> Unit {
      if self.code is Some(code) && WaitableStatus::decode(code) is Cancelled(_) {
            return
      }
      self.code = Some((self.vtable.cancel_read)(self.handle))
}

///|
pub impl[T] Waitable for StreamReader[T] with drop(self) -> Bool {
      _async_debug("stream-reader-drop(\{self.handle})")
      if self.dropped {
            return false
      }
      (self.vtable.drop_readable)(self.handle)
      self.dropped = true
      for ptr in self.memory_refs {
            (self.vtable.free)(ptr)
      }
      true
}

///|
pub impl[T] Waitable for StreamReader[T] with done(self) -> Bool {
      match self.code {
            Some(c) =>
            match WaitableStatus::decode(c) {
                  Completed(_) | Dropped(_) | Cancelled(_) => true
                  Blocking => false
            }
            None => false
      }
}

///|
pub fn[T] StreamReader::new(
handle : Int,
vtable : StreamVTable[T],
) -> StreamReader[T] {
      { handle, vtable, code: None, memory_refs: [], dropped: false }
}

///|
pub async fn[T] StreamReader::read(
self : StreamReader[T],
buffer : FixedArray[T],
offset? : Int = 0,
length : Int,
) -> Int {
      // register this waitable to the current task
      let task = current_task()
      task.add_waitable(self, current_coroutine())
      defer task.remove_waitable(self)
      let buf_ptr = (self.vtable.malloc)(length)
      self.code = Some((self.vtable.read)(self.handle, buf_ptr, length))
      _async_debug("stream-read(\{self.handle}) -> \{self.code.unwrap()}")
      for {
            let status = WaitableStatus::decode(self.code.unwrap())
            match status {
                  Completed(n) => {
                        let read_result = (self.vtable.lift)(buf_ptr, n)
                        for i in 0..<n {
                              buffer[offset + i] = read_result[i]
                        }
                        return n
                  }
                  Cancelled(n) | Dropped(n) => {
                        let read_result = (self.vtable.lift)(buf_ptr, n)
                        for i in 0..<n {
                              buffer[offset + i] = read_result[i]
                        }
                        raise StreamCancelled::StreamCancelled((n, Cancelled::Cancelled))
                  }
                  Blocking => suspend()
            }
      }
}

///|
pub struct StreamWriter[T] {
      handle : Int
      vtable : StreamVTable[T]
      mut code : Int?
      mut dropped : Bool
      memory_refs : Array[Int]
}

///|
pub impl[T] Waitable for StreamWriter[T] with update(self, code~ : Int) -> Unit {
      self.code = Some(code)
}

///|
pub impl[T] Eq for StreamWriter[T] with equal(self, other) -> Bool {
      self.handle == other.handle
}

///|
pub impl[T] Waitable for StreamWriter[T] with handle(self) -> Int {
      self.handle
}

///|
pub impl[T] Waitable for StreamWriter[T] with cancel(self) -> Unit {
      if self.code is Some(code) && WaitableStatus::decode(code) is Cancelled(_) {
            return
      }
      self.code = Some((self.vtable.cancel_write)(self.handle))
}

///|
pub impl[T] Waitable for StreamWriter[T] with drop(self) -> Bool {
      _async_debug("stream-writer-drop(\{self.handle})")
      let task = current_task()
      let coro = task.children.get(self.handle)
      if coro is Some((_, coro)) {
            coro.cancel()
            coro.wake()
      }
      if self.dropped {
            return false
      }
      (self.vtable.drop_writable)(self.handle)
      self.dropped = true
      for ptr in self.memory_refs {
            (self.vtable.free)(ptr)
      }
      true
}

///|
pub impl[T] Waitable for StreamWriter[T] with done(self) -> Bool {
      match self.code {
            Some(c) =>
            match WaitableStatus::decode(c) {
                  Completed(_) | Dropped(_) | Cancelled(_) => true
                  Blocking => false
            }
            None => false
      }
}

///|
pub fn[T] StreamWriter::new(
handle : Int,
vtable : StreamVTable[T],
) -> StreamWriter[T] {
      { handle, vtable, code: None, memory_refs: [], dropped: false }
}

///|
pub async fn[T] StreamWriter::write(
self : StreamWriter[T],
buffer : FixedArray[T],
) -> Int {
      // register this waitable to the current task
      let task = current_task()
      task.add_waitable(self, current_coroutine())
      defer task.remove_waitable(self)
      let write_buf = (self.vtable.lower)(buffer)
      self.code = Some((self.vtable.write)(self.handle, write_buf, buffer.length()))
      for {
            let status = WaitableStatus::decode(self.code.unwrap())
            match status {
                  Completed(n) => return n
                  Cancelled(n) | Dropped(n) =>
                  raise StreamCancelled::StreamCancelled((n, Cancelled::Cancelled))
                  Blocking => suspend()
            }
      }
}
