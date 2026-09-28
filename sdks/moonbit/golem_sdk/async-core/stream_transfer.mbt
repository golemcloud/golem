///|
#internal(wit_bindgen, "generated binding code only")
pub fn[X] Stream::transfer(self : Stream[X]) -> Stream[X] {
  match self.inner {
    Outgoing(state) if state.val.pipe is None => {
      let transferred = Stream::{
        inner: Outgoing({
          val: {
            producer: state.val.producer,
            cleanup: state.val.cleanup,
            on_unstarted_drop: state.val.on_unstarted_drop,
            pipe: None,
          },
        }),
      }
      state.val.producer = None
      state.val.cleanup = None
      state.val.on_unstarted_drop = None
      transferred
    }
    _ => self
  }
}
