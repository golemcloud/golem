///|
fn f(s : String) -> Unit {
  match s {
    [.. False, .. rest, .. @pkg.True ] => println(rest)
    _ => println(s)
  }
}
