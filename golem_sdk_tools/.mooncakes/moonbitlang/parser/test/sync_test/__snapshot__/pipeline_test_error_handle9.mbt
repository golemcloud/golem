pub(all) suberror FooError derive(Show)

fn foo(r : Ref[Int], err : Bool) -> Unit raise FooError {
  if err {
    raise FooError
  }
  r.val = 1
}

fn main {
  let r = try? foo(Ref::new(0), false)
  println(r)
}
