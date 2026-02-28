struct T(Int)

fn init {
  let a: FixedArray[_] = [T::T(1)]
  match a[0] {
    x => println(x.0)
  }
}
