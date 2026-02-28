fn neg(a : Float) -> Float = "%f32.neg"
fn add(a : Float, b : Float) -> Float = "%f32.add"
fn sub(a : Float, b : Float) -> Float = "%f32.sub"
fn mul(a : Float, b : Float) -> Float = "%f32.mul"
fn div(a : Float, b : Float) -> Float = "%f32.div"
fn sqrt(a : Float) -> Float = "%f32.sqrt"
fn eq(a : Float, b : Float) -> Bool = "%f32.eq"
fn ne(a : Float, b : Float) -> Bool = "%f32.ne"
fn compare(a : Float, b : Float) -> Int = "%f32.compare"
fn i32_to_f32(a : Int) -> Float = "%i32.to_f32"
// fn i64_to_f32(a : Int64) -> Float = "%i64.to_f32"
fn f64_to_f32(a : Double) -> Float = "%f64.to_f32"
fn f32_to_f64(a : Float) -> Double = "%f32.to_f64"
fn byte_to_f32(a : Byte) -> Float = "%byte.to_f32"
fn u32_to_f32(a : UInt) -> Float = "%u32.to_f32"
// fn u64_to_f32(a : UInt64) -> Float = "%u64.to_f32"
fn f32_to_i32_reinterpret(a : Float) -> Int = "%f32.to_i32_reinterpret"
fn i32_to_f32_reinterpret(a : Int) -> Float = "%i32.to_f32_reinterpret"
fn f32_to_i32(a : Float) -> Int = "%f32.to_i32"
fn f32_to_u32(a : Float) -> UInt = "%f32.to_u32"

fn show(a : Float) -> Unit {
  f32_to_f64(a).to_string() |> println()
}

fn checki(a : Int, b : Int) -> Unit {
  if a == b {
    println("ok")
  } else {
    println("fail")
  }
}

fn checkf(a : Float, b : Float) -> Unit {
  if eq(a,b) {
    println("ok")
  } else {
    println("fail")
  }
}

fn init {
  let a : Float = 3.14  
  show(a)
  show(neg(a))
  show(neg(-2.7123))

  show(add(a, a))
  show(add(a, 0.5))

  show(sub(a, a))
  show(sub(a, 0.5))
  
  show(mul(a, a))
  show(mul(a, 2.0))
  
  show(div(a, a))
  show(div(4.0, 2.0))

  show(sqrt(4.0))
  show(sqrt(0.36))
  show(sqrt(1024.0))

  println(eq(a, a))
  println(ne(a, a))
  println(compare(a, a))
  
  show(i32_to_f32(42))
  // show(i64_to_f32(42L))
  show(f64_to_f32(42.0))
  println(f32_to_f64(42.12345))
  show(byte_to_f32(b'\x42'))
  show(u32_to_f32(42U))
  // show(u64_to_f32(42UL))
  
  f32_to_i32_reinterpret(0.0) |> checki(0)
  f32_to_i32_reinterpret(-0.0) |> checki(0x80000000)
  f32_to_i32_reinterpret(1.0) |> checki(1065353216)
  f32_to_i32_reinterpret(3.1415926) |> checki(1078530010)
  
  i32_to_f32_reinterpret(0) |> checkf(0)
  i32_to_f32_reinterpret(0x80000000) |> checkf(-0.0)
  i32_to_f32_reinterpret(123456789) |> checkf(0x1.b79a2ap-113)

  println(f32_to_i32(3.14))
  println(f32_to_u32(3.14))
}

suberror FloatErr {
  FloatErr(Int)
}

fn ret_float_with_err(b : Bool) -> Float raise Error {
  if b { raise FloatErr(0) }
  3.14
}

let global_float : Float = (3.14 : Float)

fn test_local_float() -> Unit {
  let a : Float = 3.14
  show(a)
}

fn test_float_array() -> Unit {
  let a : Array[Float] = [1.0, 2.0, 3.0]
  let mut sum : Float = 0.0
  for i = 0; i < a.length(); i = i + 1 {
    sum = add(sum, a[i])
  }
  show(sum)
}

fn init {
  (try { ret_float_with_err(false) } catch { _ => 0.0 }) |> show()
  (try { ret_float_with_err(true) } catch { _ => 0.1 }) |> show()

  show(global_float)
  test_local_float()
  test_float_array()
}



