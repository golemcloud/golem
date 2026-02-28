// type Iter[X] ((X) -> Unit raise?) -> Unit raise?

// In this setup, `IterResult` can be avoided,
// but the `Iter` itself MUST NOT raise any error,
// only the `for .. in`/`each` etc. is allowed to raise error,
// and the `Iter` itself can only tunnel those error.

pub fn[X] from_array(arr : Array[X]) -> (
  (X) -> Unit raise?
) -> Unit raise? {
  fn (yield_) {
    for x in arr {
      yield_(x)
    }
  }
}

pub fn[X] each(
  it : ((X) -> Unit raise?) -> Unit raise?,
  f : (X) -> Unit raise?
) -> Unit raise? {
  it(f)
}

pub fn[X] collect(
  it : ((X) -> Unit raise?) -> Unit raise?
) -> Array[X] raise? {
  let result = []
  each(it, result.push(_))
  result
}

pub fn[X, Y] map(
  it : ((X) -> Unit raise?) -> Unit raise?,
  f : (X) -> Y
) -> ((Y) -> Unit raise?) -> Unit raise? {
  yield_ => it(x => yield_(f(x)))
}

pub fn[X] filter(
  it : ((X) -> Unit raise?) -> Unit raise?,
  f : (X) -> Bool
) -> ((X) -> Unit raise?) -> Unit raise? {
  yield_ => it(fn (x) {
    if f(x) {
      yield_(x)
    }
  })
}

priv suberror IterTake

pub fn[X] take(
  it : ((X) -> Unit raise) -> Unit raise,
  n : Int
) -> ((X) -> Unit raise?) -> Unit raise? {
  fn (yield_) {
    let mut err_from_yield = None
    let mut i = 0
    it(fn (x) {
      if i > n { raise IterTake }
      try yield_(x) catch {
        err => {
          err_from_yield = Some(err)
          raise IterTake
        }
      }
    }) catch {
      IterTake => ()
      _ =>
        // The invariant is, an iterator never raise error by itself,
        // and can only tunnel error raised by its callback.
        // The callback above only raise `IterTake`,
        // and error from `yield_` is handled by `err_from_yield`,
        // so the error here must be `IterTake`
        panic()
    }
    // tunnel error raised by `yield_`
    match err_from_yield {
      None => ()
      Some(err) => raise err
    }
  }
}

priv suberror IterFind

pub fn[X] find(
  it : ((X) -> Unit raise) -> Unit raise,
  f : (X) -> Bool
) -> X? {
  let mut result = None
  try {
    it(fn (x) {
      if f(x) {
        result = Some(x)
        raise IterFind
      }
    })
  } catch {
    IterFind => ()
    _ => panic()
  }
  result
}

fn it() -> ((Int) -> Unit raise?) -> Unit raise? {
  Array::makei(100, i => i)
  |> from_array
  |> take(50)
  |> map(x => x * x)
  |> filter(x => x % 2 == 0)
  |> take(10)
}

fn main {

  println(collect(it()))
  println(find(it(), x => x > 42))
}
