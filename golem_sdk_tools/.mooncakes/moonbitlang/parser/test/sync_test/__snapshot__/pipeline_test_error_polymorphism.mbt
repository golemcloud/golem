suberror MyErr {
  MyErr(Int)
} derive(Show)

fn[X] each(arr : Array[X], f : (X) -> Unit raise?) -> Unit raise? {
  for x in arr {
    f(x)
  }
}

fn no_error() -> Unit {
  each([ 1, 2, 3 ], println)
}

fn init {
  println("===")
  no_error()
}

fn has_error() -> Unit raise {
  each([ 1, 2, 3 ], fn (x) {
    if x % 2 == 0 {
      raise MyErr(x)
    } else {
      println(x)
    }
  })
}

fn init {
  println("===")
  (try? has_error()) |> println
}

fn rethrow(f : () -> Int raise?) -> Int raise? {
  try {
    f()
  } catch {
    err => raise err
  }
}

fn init {
  println("===")
  println(try? rethrow(fn () { raise MyErr(42) }))
  println(rethrow(fn () { 42 }))
}

fn no_error_in_array(arr : Array[Int], f : (Int) -> Unit raise?) -> Int {
  let errs = []
  for x in arr {
    try {
      f(x)
    } catch {
      err => errs.push(err)
    }
  }
  errs.length()
}

fn init {
  println("===")
  no_error_in_array([ 1, 2, 3, 4 ], fn (x) {
    if x % 2 == 0 {
      raise MyErr(x)
    }
  })
  |> println
  no_error_in_array([ 1, 2, 3, 4 ], _x => ())
  |> println
}
