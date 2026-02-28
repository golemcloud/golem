async fn f(cb : async () -> Int) -> Int {
  cb()
}

fn main {
  async fn g() {}
  f(async fn () { g!(); 42 })
  x.f()
  x |> f()
}

fn h(
  f1 : async () -> Int,
  f2 : async () -> Int raise,
) -> Unit {
}
