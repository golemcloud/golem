// numeric type
fn init {
  let xs: UninitializedArray[Int] = UninitializedArray::make(10)
  xs[0] = 42
  println(xs[0])
  // println(xs[1]) // default to 0
  println(xs.length())
}

fn init {
  let xs: UninitializedArray[Double] = UninitializedArray::make(10)
  xs[0] = 42.0
  println(xs[0])
  // println(xs[1]) // default to 0.0 in wasm(-gc) backend, Nan in js backend
  println(xs.length())
}

// reference type
fn init {
  let xs: UninitializedArray[(Int, Int)] = UninitializedArray::make(10)
  xs[0] = (0, 42)
  println(xs[0].1)
  println(xs.length())
}

pub fn[T] zero_out(arr: UninitializedArray[T], index: Int) -> Unit = "%fixedarray.set_default"

fn init {
  let xs: UninitializedArray[Int] = UninitializedArray::make(10)
  xs[1] = 42
  println(xs[1])
  zero_out(xs, 1)
  println(xs[1])
}

pub fn[T] null_out(arr: UninitializedArray[T], index: Int) -> Unit = "%fixedarray.set_null"
fn init {
  let xs: UninitializedArray[Int] = UninitializedArray::make(10)
  xs[1] = 42
  println(xs[1])
  null_out(xs, 1)
  println(xs[1])
}

// recursive newtype, aka any ref
struct RecNewType(RecNewType?) derive(Show)

fn init {
  let xs : UninitializedArray[RecNewType] = UninitializedArray::make(10)
  xs[0] = Some(Some(None))
  println(xs[0])
}

#external
type Extern

fn init {
  let xs : UninitializedArray[Extern] = UninitializedArray::make(10)
  println(xs.length())
}
