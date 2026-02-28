//!build: -w -20

fn init {
  for x in [ 1, 2, 3, 4 ].iter() {
    println(x)
  }
  for k, v in { "x": 1, "y": 2, "z": 3 }.iter2() {
    println("\{k} => \{v}")
  }
}

// control flow
fn f1(mode : Int) -> String {
  (for x in [ 1, 2, 3, 42 ].iter() {
    if x == 42 {
      if mode == 0 {
        return "return"
      } else if mode == 1 {
        break 42
      }
    }
  } else {
    0
  })
  |> Int::to_string
}

fn init {
  for mode in [ 0, 1, 2 ].iter() {
    println(f1(mode))
  }
}

// error handling
fn f2(mode : Int) -> String raise {
  for x in [ 1, 2, 3, 42 ].iter() {
    if x == 42 {
      if mode == 0 {
        return "return"
      } else if mode == 1 {
        break "break"
      } else if mode == 2 {
        raise Failure::Failure("error")
      } else {
        continue
      }
    }
  } else {
    "continue"
  }
}

fn init {
  for mode in [ 0, 1, 2, 3 ].iter() {
    println(try? f2(mode))
  }
}


// nested for
fn init {
  fn it() {
    let mut i = 0
    let mut j = 0
    let arr1 = [ 1, 2, 3 ]
    let arr2 = [ 3, 2, 1 ]
    Iter::new(() => {
      guard i < 3 else { None }
      let x = arr1[i]
      let y = arr2[j]
      if j == 2 {
        j = 0
        i += 1
      } else {
        j += 1
      }
      Some((x, y))
    })
  }
  it().each(println)
  println(it().find_first(fn (t) { match t { (x, y) => x == 3 && y == 2 } }))
  println(it().find_first(fn (t) { match t { (x, _) => x > 3 } }))
}

fn init {
  let it = [0, 1, 2].iter()
  // if the iterated expression is already an `Iter`,
  // don't insert `.iter()` call
  for x in it {
    println(x)
  }
}

fn init {
  let arr = [ 1, 2, 3 ]
  // `Iter::iter2`
  for i, x in arr.iter() {
    println("\{i} => \{x}")
  }
}
