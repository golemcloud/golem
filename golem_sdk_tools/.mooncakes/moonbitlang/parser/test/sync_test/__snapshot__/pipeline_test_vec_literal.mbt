fn[T: Show] print_vec(v: Array[T]) -> Unit {
  println(v)
}

fn init {
  print_vec([1, 2, 3])
  print_vec([(1, 2), (3, 4)])
  let v: Array[_] = [1.0, 2.0, 3.0]
  v.push(4.0)
  print_vec(v)
  let vv: Array[_] = [v, [5.0]]
  print_vec(vv)
}
