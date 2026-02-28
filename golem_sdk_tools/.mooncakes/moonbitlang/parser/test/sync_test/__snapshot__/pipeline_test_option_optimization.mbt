fn test_char(xs : Array[Char?]) -> Unit {
  println(xs)
}

fn test_bool(xs : Array[Bool?]) -> Unit {
  println(xs)
}

fn test_byte(xs : Array[Byte?]) -> Unit {
  println(xs)
}

fn test_byte2(xs : Array[Byte]) -> Unit {
  println(xs)
}

fn test_unit(xs : Array[Unit?]) -> Unit {
  println(xs)
}

fn test_int(xs : Array[Int?]) -> Unit {
  println(xs)
}

fn test_uint(xs : Array[UInt?]) -> Unit {
  println(xs)
}

fn test_int64(xs : Array[Int64?]) -> Unit {
  println(xs)
}

fn test_uint64(xs : Array[UInt64?]) -> Unit {
  println(xs)
}

fn test_float(xs : Array[Float?]) -> Unit {
  println(xs)
}

fn test_double(xs : Array[Double?]) -> Unit {
  println(xs)
}

fn test_option(xs : Array[Int??]) -> Unit {
  println(xs)
}

fn test_bytes(xs : Array[Bytes?]) -> Unit {
  println(xs)
}

fn test_tuple(xs : Array[(Int, Int)?]) -> Unit {
  println(xs)
}

fn test_fixedarray(xs : Array[FixedArray[Int]?]) -> Unit {
  println(xs)
}

fn test_trait_obj(xs : Array[&Show?]) -> Unit {
  println(xs)
}

struct A((A) -> Int)

fn test_any(xs : Array[A?]) -> Unit {
  for a in xs {
    match a {
      Some(a) => println((a.0)(a))
      None => ()
    }
  }
}

fn test_any2(xs : Array[A]) -> Unit {
  for a in xs {
    println((a.0)(a))
  }
}

fn test_func(xs : Array[((Int) -> Int)?]) -> Unit {
  for f in xs {
    match f {
      Some(f) => println(f(42))
      None => ()
    }
  }
}

fn main {
  test_char([Some('a'), None, Some('b')])
  test_bool([Some(true), None, Some(false)])
  test_byte([Some(b'a'), None, Some(b'b')])
  test_byte2([b'a', b'b'])
  test_unit([Some(()), None, Some(())])
  test_int([Some(1), None, Some(2)])
  test_uint([Some(1U), None, Some(2U)])
  test_int64([Some(1L), None, Some(2L)])
  test_uint64([Some(1UL), None, Some(2UL)])
  test_float([Some(1.0), None, Some(2.0)])
  test_double([Some(1.0), None, Some(2.0)])
  test_option([Some(Some(1)), None, Some(Some(2))])
  test_bytes([Some(b"abc"), None, Some(b"def")])
  test_tuple([Some((1, 2)), None, Some((3, 4))])
  test_fixedarray([Some([1, 2, 3]), None, Some([4, 5, 6])])
  test_trait_obj([Some(123), None, Some("def")])
  test_any([Some(A(fn(_x) { 42 })), None, Some(A(fn(_x) { 43 }))])
  test_any2([A(fn(_x) { 44 }), A(fn(_x) { 45 })])
  test_func([Some(fn(x) { x }), None, Some(fn(x) { x + 1 })])
}
