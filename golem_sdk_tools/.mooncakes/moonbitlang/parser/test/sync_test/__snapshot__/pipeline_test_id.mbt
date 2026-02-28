fn[T] id(x: T) -> T { x }

fn init {
  let a = id(1)
  let b = id('c')
  let c = id((["xxx"]: FixedArray[_]))
}

fn[Y] use_id(x: Y) -> Unit {
  let d = id(x)
  let e = id(Option::Some(x))
}
