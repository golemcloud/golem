suberror StringError {
  StringError(String)
} derive(Show)

suberror FloatError {
  FloatError(Float)
} derive(Show)

trait TF {
  f1(Self) -> Float raise = _
  f2(Self) -> Float raise FloatError = _
}

impl TF for Int with f1(x) -> Float raise {
  if x >= 0 {
    Float::from_int(x)
  } else {
    raise StringError("x is negative")
  }
}

impl TF for Int with f2(x) -> Float raise FloatError {
  if x >= 0 {
    Float::from_int(x)
  } else {
    raise FloatError(Float::from_int(x))
  }
}

fn use_tf(x: &TF) -> Unit raise {
  let a = x.f1()
  println(a)
  let b = try! x.f2()
  println(b)
}

fn init {
  try! use_tf(42)
}

impl TF with f1(x) -> Float raise {
  if (a) { raise StringError("err") }
  42
}

impl TF with f2(x) -> Float raise FloatError {
  if (a) { raise FloatError(-1.0) }
  42
}

impl TF for Int64 with f1(x) -> Float raise {
  if (a) { raise StringError("err") }
  Float::from_int64(x)
}

impl TF for Int64 with f2(x) -> Float raise FloatError {
  if (a) { raise FloatError(-1.0) }
  Float::from_int64(x)
}

impl TF for Double with f1(x) {
  if (a) { raise StringError("err") }
  Float::from_double(x)
}

impl TF for Double with f2(x) {
  if (a) { raise FloatError(-1.0) }
  Float::from_double(x)
}

impl TF for String

fn init {
  let a = try! TF::f1("xxx")
  println(a)
  try! use_tf(42L)
  try! use_tf(42.0)
}

let a = false
