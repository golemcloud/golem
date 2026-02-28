fn run_async(f : async () -> Unit noraise) -> Unit = "%async.run"

///|
fn ff(a : async () -> Int raise) -> Unit {
  run_async(() => println(try? a()))
}

///|
fn main {
  ff(async fn() noraise { 42 })
}
