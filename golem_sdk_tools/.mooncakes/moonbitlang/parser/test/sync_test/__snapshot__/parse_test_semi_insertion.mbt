
fn test1() {
  let x = 1
  f()
  A(1)
  1()
  "abc\{x}"
  "def"
  if true { 1 } else { 2 }
  match x { None => 1 }
  while true {
    abc

    return
    // the user can insert many comment;

    // and newlines after the initial newline

    // and ASI should still work
    break
    continue
  }

  let chain = x
    .method1()
    .method2(
      // comment for first argument
      arg1,
      arg2 // no trailing comma
    )

    .method3()

  let record = {
    x,
    y: 1 // no trailing comma
  }
  let record_pun = {
    x: 1,
    y
  }
  {
    x
    y
  } // and statement block should work as usual

  let array = [
    elem1,
    elem2
      .method()
    // no trailing comma
  ]

  let tuple = (
    x, y,
    z, w
  )

  let type_annotation = 1 + (
    (fn (x) {
      veryLooooooooooooooongFunction()
    }) : Int
  )

  // currently we force users to write like this
  let operator = expr1 +
    expr2 *
    expr3 &&
    expr4

  let _ = expr1 +
    (if cond { abc } else { def })

  let _ = expr1 +
    (match cond { pat => body })

  let _ = 42
  -negative

  if true {
  }
  else { // else can occur on its own line
  }
}

fn error() {
  let op = expr1
    * expr2
    - expr3 // treated as a new statement
}
