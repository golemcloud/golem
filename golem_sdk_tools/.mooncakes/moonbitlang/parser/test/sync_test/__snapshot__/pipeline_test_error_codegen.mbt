fn f() -> Unit {}
fn hof(f : () -> Unit raise Error) -> Unit raise Error { f() }

fn init {
  try {
    hof(fn () { f() })
  } catch {
    _ => ()
  }
}
