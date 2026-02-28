fn init {
  fn is_palindrome(s: ArrayView[Int]) -> Bool {
    match s {
      [] => true
      [_] => true
      [x, .. rest, y] => x == y && is_palindrome(rest)
    }
  }
  let v1 = [1, 2, 3, 4, 5]
  println(is_palindrome(v1[:]))
  let v2 = [1, 2, 3, 2, 1]
  println(is_palindrome(v2[:]))
}


fn init {
  fn is_palindrome(s: Array[Int]) -> Bool {
    loop s[:] {
      [] => true
      [_] => true
      [x, .. rest, y] => {
        if x == y {
          continue(rest)
        } else {
          false
        }
    }
  }
  }
  let v1 = [1, 2, 3, 4, 5]
  println(is_palindrome(v1))
  let v2 = [1, 2, 3, 2, 1]
  println(is_palindrome(v2))
}
