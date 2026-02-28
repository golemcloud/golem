struct T { val: Int }

fn init {
  let x: T = if 1 > 2 { { val: 3 } } else { { val: 4 } }
  println(x.val)
}

fn f(a: Int?) -> Unit {
  let x : T = match a {
    Some(i) => { { val: i } }
    None => { { val: 0 } }
  }
  println(x.val)
}

fn init {
  f(Some(4))
}
