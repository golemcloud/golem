suberror ConstantError
suberror PayloadError {
  PayloadError(Int)
}

fn f1() -> Unit raise ConstantError {
  raise ConstantError
}

fn f2() -> Unit raise PayloadError {
  raise PayloadError(42)
}

fn f3(coin : Bool) -> Unit raise Error {
  if coin {
    raise ConstantError
  } else {
    raise PayloadError(42)
  }
}

fn init {
  try {
    f1()
  } catch {
    ConstantError => println("ConstantError")
  }
}

fn init {
  try {
    f2()
  } catch {
    PayloadError(x) => println(x)
  }
}

fn init {
  try {
    f3(true)
  } catch {
    ConstantError => println("ConstantError")
    PayloadError(x) => println(x)
    _ => panic()
  }
}

fn init {
  try {
    f3(false)
  } catch {
    ConstantError => println("ConstantError")
    PayloadError(x) => println(x)
    _ => panic()
  }
}
