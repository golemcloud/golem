fn String::local_get(s: String, i: Int) -> Int? {
  if i >= 0 && i < s.length() {
    Some(s.code_unit_at(i).to_int())
  } else {
    None
  }
}

///|
fn main {
  let a = "hello"
  for i = 0; a.local_get(i) is Some(x); i = i + 1 {
    println(x)
  }
}

///|
fn test_is_in_for_cond_with_array() -> Unit {
  let arr = [1, 2, 3, 4, 5]
  for i = 0; arr.get(i) is Some(x); i = i + 1 {
    println(x)
  }
}

///|
fn test_is_in_for_cond_with_empty_string() -> Unit {
  let s = ""
  for i = 0; s.local_get(i) is Some(x); i = i + 1 {
    println(x)
  }
  println("done")
}

///|
fn test_is_in_for_cond_with_option_values() -> Unit {
  let opts = [Some(1), Some(2), None, Some(4), Some(5)]
  for i = 0; opts.get(i) is Some(opt); i = i + 1 {
    match opt {
      Some(x) => println(x)
      None => println("none")
    }
  }
}

///|
fn test_is_in_for_cond_nested() -> Unit {
  let matrix = [["a", "b"], ["c", "d"], ["e", "f"]]
  for i = 0; matrix.get(i) is Some(row); i = i + 1 {
    for j = 0; row.get(j) is Some(x); j = j + 1 {
      println(x)
    }
  }
}

///|
fn test_is_in_for_cond_with_break() -> Unit {
  let s = "hello world"
  for i = 0; s.local_get(i) is Some(x); i = i + 1 {
    if x == ' ' {
      break
    }
    println(x)
  }
}

///|
fn test_is_in_for_cond_with_continue() -> Unit {
  let s = "hello"
  for i = 0; s.local_get(i) is Some(x); i = i + 1 {
    if x == 'l' {
      continue
    }
    println(x)
  }
}

///|
enum MyOption[T] {
  MySome(T)
  MyNone
}

///|
fn get_item(arr : Array[Int], i : Int) -> MyOption[Int] {
  if i >= 0 && i < arr.length() {
    MySome(arr[i])
  } else {
    MyNone
  }
}

///|
fn test_is_in_for_cond_with_custom_type() -> Unit {
  let arr = [10, 20, 30]
  for i = 0; get_item(arr, i) is MySome(x); i = i + 1 {
    println(x)
  }
}

///|
enum Result[T, E] {
  Ok(T)
  Err(E)
}

///|
fn get_result(i : Int) -> Result[Int, String]? {
  if i < 3 {
    Some(Ok(i * 10))
  } else if i < 5 {
    Some(Err("error"))
  } else {
    None
  }
}

///|
fn test_is_in_for_cond_with_multiple_bindings() -> Unit {
  for i = 0; get_result(i) is Some(res); i = i + 1 {
    match res {
      Ok(x) => println(x)
      Err(e) => println(e)
    }
  }
}

///|
fn test_is_in_for_cond_with_accumulator() -> Unit {
  let s = "12345"
  let mut sum = 0
  for i = 0; s.local_get(i) is Some(x); i = i + 1 {
    let digit = x - '0'.to_int()
    sum = sum + digit
  }
  println(sum)
}

///|
fn test_is_in_for_cond_with_unicode() -> Unit {
  let s = "你好世界"
  for i = 0; s.local_get(i) is Some(x); i = i + 1 {
    println(x)
  }
}

///|
fn test_is_in_for_cond_with_guard_in_body() -> Unit {
  let arr = [Some(1), Some(2), None, Some(4), Some(5)]
  for i = 0; arr.get(i) is Some(opt); i = i + 1 {
    guard opt is Some(x) else { continue }
    println(x)
  }
}

///|
fn test_is_in_for_cond_with_and() -> Unit {
  let arr = [Some(1), Some(2), None, Some(4), Some(5)]
  for i = 0; arr.get(i) is Some(opt) && opt is Some(x); i = i + 1 {
    println(x)
  }
}

///|
fn init {
  test_is_in_for_cond_with_array()
  test_is_in_for_cond_with_empty_string()
  test_is_in_for_cond_with_option_values()
  test_is_in_for_cond_nested()
  test_is_in_for_cond_with_break()
  test_is_in_for_cond_with_continue()
  test_is_in_for_cond_with_custom_type()
  test_is_in_for_cond_with_multiple_bindings()
  test_is_in_for_cond_with_accumulator()
  test_is_in_for_cond_with_unicode()
  test_is_in_for_cond_with_guard_in_body()
  test_is_in_for_cond_with_and()
}
