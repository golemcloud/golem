
fn init {
  let _ = 1 as Show
  let _ = 1 + x.f().g() as @pkg.Trait
  let _ = f(x) as I * 3
  let _ = f(1 as Show, 2 as Hash)

  let _ = 1 as Show.f()
  let _ = 1 as Show as Show
  let _ = 1 as Show(x)
  let _ = 1 as Show?

  let _ = (1 as Show).f()
  let _ = (1 as Show) as Show
  let _ = (1 as Show)(x)
  let _ = (1 as Show)?

  let _ = 1 as Show + Debug // not supported yet
  let _ = 1 as Show + 1 // syntax error
  let _ = (1 as Show) + 1 // fine
}

fn use_object(obj : &@builtin.Show) -> &Show { obj }

trait I {}

fn &I::meth() -> Unit {}

fn init {
  &I::meth()
}
