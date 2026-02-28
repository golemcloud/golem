fn init {
  println("==== basic ====")
  defer println("first defer")
  defer println("second defer")
  println("code finished")
}

suberror Err derive(Show)

fn function_with_error(coin : Bool) -> Unit raise Err {
  if coin {
    raise Err
  }
}

fn basic_err(coin : Bool) -> Unit raise {
  defer println("defer before error")
  function_with_error(coin)
}

fn init {
  println("==== basic err, normal return ====")
  ignore(try? basic_err(false))
  println("==== basic err, raise error ====")
  ignore(try? basic_err(true))
}

fn basic_try(coin : Bool) -> Unit raise {
  defer println("defer outside try")
  try {
    defer println("defer inside try")
    function_with_error(coin)
  } catch {
    err => {
      println("received error \{err}")
      raise err
    }
  } noraise {
    _ => println("return normally")
  }
}

fn init {
  println("==== basic try, normal return ====")
  ignore(try? basic_try(false))
  println("==== basic try, raise error ====")
  ignore(try? basic_try(true))
}

fn lexical_scope(coin : Bool) -> Unit {
  defer println("outer defer")
  if (coin) {
    defer println("inner defer for branch 1")
    println("branch 1 finished")
  } else {
    defer println("inner defer for branch 2")
    println("branch 2 finished")
  }
  defer println("defer after if")
  println("after if")
}

fn init {
  println("==== lexical scope, branch 1 ====")
  lexical_scope(true)
  println("==== lexical scope, branch 2 ====")
  lexical_scope(false)
}

fn basic_loop() -> Unit {
  defer println("defer outside loop")
  for i in 0..<3 {
    defer println("defer for loop #\{i}")
    println("loop iteration #\{i}")
  }
}

fn init {
  println("==== basic loop ====")
  basic_loop()
}

fn loop_control(cmd : String) -> Unit {
  defer println("defer outside loop")
  for _ in 0..<1 {
    defer println("defer inside loop")
    match cmd {
      "continue" => {
        defer println("defer before `continue`")
        continue { println("eval argument of continue"); 1 }
      }
      "break" => {
        defer println("defer before `break`")
        break println("eval argument of break")
      }
      _ => panic()
    }
  } else {
    println("after loop")
  }
}

fn init {
  println("==== basic `continue` ====")
  loop_control("continue")
  println("==== basic `break` ====")
  loop_control("break")
}

fn labelled_loop_control(cmd : String) -> Unit {
  defer println("outer defer")
  for _ in 0..<1 {
    defer println("defer for loop 1")
    l1~: for _ in 0..<1 {
      defer println("defer for loop 2")
      for _ in 0..<1 {
        defer println("defer for loop 3")
        match cmd {
          "continue" => continue l1~
          "break" => break l1~
          _ => panic()
        }
      } else {
        println("after loop 3")
      }
    } else {
      println("after loop 2")
    }
  } else {
    println("after loop 1")
  }
}

fn init {
  println("==== labelled `continue` ====")
  labelled_loop_control("continue")
  println("==== labelled `break` ====")
  labelled_loop_control("break")
}

fn basic_return(ret : Bool) -> Unit {
  defer println("first defer")
  {
    defer println("second defer")
    {
      defer println("third defer")
      if ret {
        return println("eval return value")
      }
      println("end of third scope")
    }
    println("end of second scope")
  }
  println("end of first scope")
}

fn init {
  println("==== basic_return, normal case ====")
  basic_return(false)
  println("==== basic_return, return case ====")
  basic_return(true)
}

fn basic_for_in(cmd : String, it : Iter[Int]) -> Unit raise {
  defer println("outer-most defer")
  l1~: for _ in 0..<1 {
    defer println("defer for outer loop")
    for _ in it {
      defer println("defer inside `for .. in`")
      match cmd {
        "continue" => continue
        "break" => break println("eval argument of break")
        "continue-outer" => continue l1~ { println("eval argument of labelled continue"); 1 }
        "break-outer" => break l1~ println("eval argument of labelled break")
        "raise" => function_with_error(true)
        "return" => return
        _ => println("normal fallthrough")
      }
    } else {
      println("after `for .. in`")
    }
  } else {
    println("after outer loop")
  }
}

fn init {
  let data = [ 42 ]
  println("==== `for .. in`: fallthrough ====")
  ignore(try? basic_for_in("normal", data.iter()))
  println("==== `for .. in`: continue ====")
  ignore(try? basic_for_in("continue", data.iter()))
  println("==== `for .. in`: break ====")
  ignore(try? basic_for_in("break", data.iter()))
  println("==== `for .. in`: continue to outer loop ====")
  ignore(try? basic_for_in("continue-outer", data.iter()))
  println("==== `for .. in`: break to outer loop ====")
  ignore(try? basic_for_in("break-outer", data.iter()))
  println("==== `for .. in`: error ====")
  ignore(try? basic_for_in("raise", data.iter()))
  println("==== `for .. in`: return ====")
  ignore(try? basic_for_in("return", data.iter()))
}
