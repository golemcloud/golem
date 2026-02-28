async fn[T] suspend(cb : ((T) -> Unit) -> Unit) -> T noraise = "%async.suspend"
fn run_async(f : async () -> Unit noraise) -> Unit = "%async.run"

let coros : @deque.Deque[(Unit) -> Unit] = @deque.new()

async fn pause() -> Unit noraise {
  suspend(k => coros.push_back(k))
}

fn spawn(f : async () -> Unit noraise) -> Unit {
  coros.push_back(_ => run_async(f))
}

fn run_all() -> Unit {
  while coros.pop_front() is Some(k) {
    k(())
  }
}

fn main {
  let map = { "x": 1, "y": 2, "z": 3 }
  let str = "字😅↑"
  spawn(fn() {
    for k, v in map {
      println("from map: \{k} => \{v}")
      pause()
    }
  })
  spawn(fn() {
    for c in str {
      println("from str: \{c}")
      pause()
    }
  })
  run_all()
}
