suberror OError {
  OError(Option[Int])
} derive(Show)

fn f(x: Int) -> Option[Int] raise OError {
  if x == 0 {
    raise OError(None)
  } else if x == 1 {
    raise OError(Some(1))
  } else if x == -1 {
    None
  } else {
    Some(x)
  }
}


fn f1(x: Int) -> Unit {
  try f(x) catch {
    OError(None) => println("err none")
    OError(Some(y)) => {
      println("err \{y}")
    }
  } noraise {
    None => println("ok none")
    Some(y) => {
      println("ok \{y}")
    }
  }
}

fn init {
  f1(0)
  f1(1)
  f1(-1)
  f1(42)
}

fn[X, V, E : Error] as_result(f: (X) -> V raise E) -> (X) -> Result[V, E] {
  x => {
     try f(x) catch {
       e => Err(e)
     } noraise {
       v => Ok(v)
     }
    }
}

fn init {
  println(as_result(f)(0))
  println(as_result(f)(1))
  println(as_result(f)(-1))
  println(as_result(f)(42))
}
