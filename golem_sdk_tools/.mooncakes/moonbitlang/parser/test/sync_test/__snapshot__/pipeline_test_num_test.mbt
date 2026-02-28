fn xtest_i64(x: Int64, y: Int64) -> Unit {
  println(-x)
  println(x + y)
  println(x * y)
  println(x - y)
  println(x / y)
  println(x % y)
}

fn xtest_f64(x: Double, y: Double) -> Unit {
  println(x)
  println(y)
  println(-x)
  println(x + y)
  println(x * y)
  println(x - y)
  println(x / y)
}

fn convert_from_i32(i: Int) -> Unit {
  println(i)
  println(i.to_int64())
  println(i.to_double())
}

fn convert_from_i64(i: Int64) -> Unit {
  println(i)
  println(i.to_int())
  println(i.to_double())
}

fn convert_from_f64(i: Double) -> Unit {
  println(i)
  println("!!to_int() traps here!!\n")
  // i.to_int().println_i32()
  // println(i.to_int64())
}

fn compare_i64(i1: Int64, i2: Int64) -> Unit {
  if (i1 < i2) { println("lt\n") } else { println("not lt\n") }
  if (i1 <= i2) { println("le\n") } else { println("not le\n") }
  if (i1 > i2) { println("gt\n") } else { println("not gt\n") }
  if (i1 >= i2) { println("ge\n") } else { println("not ge\n") }
  if (i1 == i2) { println("eq\n") } else { println("not eq\n") }
}

fn init {
  println("==testi64==\n")
  xtest_i64(-9223372036854775808L, 9223372036854775807L)
  xtest_i64(2147483648L, 2023L)
  println("==testf64==\n")
  xtest_f64(20.23 * 9.8, -20.23 / 9.8)
  println("==convert_from_i32==\n")
  convert_from_i32(913)
  println("==convert_from_i64==\n")
  convert_from_i64(-2023L * 2147483647L)
  println("==convert_from_f64==\n")
  convert_from_f64(-202.398 * 2147483647.0)
  println("==compare_i64==\n")
  compare_i64(9223372036854775807L, 223372036854775807L)
}

fn init {
  println("==i64_to_string==\n")
  let x = 9223372036854775807L
  let y = -9223372036854775807L
  println(x.to_string() + y.to_string())
}
