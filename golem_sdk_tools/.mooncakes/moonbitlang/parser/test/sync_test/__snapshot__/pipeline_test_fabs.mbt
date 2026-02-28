fn fabsf(value : Float) -> Float = "%f32.abs"

fn fabs(value : Double) -> Double = "%f64.abs"

fn main {
  println(fabsf(-3.14))
  println(fabs(-3.14))
  println(fabsf(-0.0).reinterpret_as_uint())
  println(fabs(-0.0).reinterpret_as_uint64())
}
