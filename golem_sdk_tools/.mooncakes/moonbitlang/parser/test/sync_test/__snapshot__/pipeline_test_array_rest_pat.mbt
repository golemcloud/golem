fn init {
  let a0 = ([] : FixedArray[Int])
  let a1 = ([0]: FixedArray[_])
  let a2 = ([0, 1]: FixedArray[_])
  let a3 = ([0, 1, 2]: FixedArray[_])
  let a4 = ([0, 1, 2, 3]: FixedArray[_])
  let len_is_2_or_3 = fn (a: FixedArray[Int], l: String) {
    match a {
      [_, _] | [_, _, _] => println("length of \{l} is 2 | 3 => yes\n")
      _ => println("length of \{l} is 2 | 3 => no\n")
    }
  }
  let first_2_is_01 = fn (a: FixedArray[Int], l: String) {
    match a {
      [0, 1, .._] => println("first two of \{l} is [0, 1] => yes\n")
      _ => println("first two of \{l} is [0, 1] => no\n")
    }
  }
  let last_1_is_2 = fn (a: FixedArray[Int], l: String) {
    match a {
      [.. _, 2] => println("last one of \{l} is 2 => yes\n")
      _ => println("last one of \{l} is 2 => no\n")
    }
  }
  let t = fn(f) {
    f(a0, "[]")
    f(a1, "[0]")
    f(a2, "[0, 1]")
    f(a3, "[0, 1, 2]")
    f(a4, "[0, 1, 2, 3]")
  }
  t(len_is_2_or_3)
  t(first_2_is_01)
  t(last_1_is_2)
}
