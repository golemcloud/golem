///|
pub(all) struct FutureR[X](async () -> X)

///|
pub(all) struct StreamR[X] {
      read : async (Int) -> ArrayView[X]?
      close : async () -> Unit
}

///|
pub(all) struct StreamW[X] {
      write : async (ArrayView[X]) -> Int
      close : async () -> Unit
}

///|
struct OutStream[X] {
      mut stream : StreamW[X]?
      mut coroutine : Coroutine?
} derive(Default)

///|
pub async fn[X] OutStream::get_stream(self : OutStream[X]) -> StreamW[X] {
      if self.stream is Some(s) {
            return s
      } else {
            guard self.coroutine is None
            self.coroutine = Some(current_coroutine())
            suspend() catch {
                  e => {
                        if self.stream is Some(s) {
                              (s.close)()
                        }
                        raise e
                  }
            }
            self.stream.unwrap()
      }
}

///|
pub fn[X] OutStream::put_stream(
self : OutStream[X],
stream : StreamW[X],
) -> Unit {
      self.stream = Some(stream)
      if self.coroutine is Some(coro) {
            coro.wake()
      }
}
