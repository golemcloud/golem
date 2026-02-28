///|
fn run_async(f : async () -> Unit noraise) -> Unit = "%async.run"

///|
let counter : Ref[Int] = Ref::new(0)

///|
#warnings("-unused_async")
async fn actually_sync() -> Unit noraise {
  counter.val += 1
}

///|
fn main {
  run_async(() => for _ in 0..<100000 {
    actually_sync()
  })
  println(counter.val)
}
