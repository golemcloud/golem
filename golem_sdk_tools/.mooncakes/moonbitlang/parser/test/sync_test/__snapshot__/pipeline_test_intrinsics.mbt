/// @intrinsic %char.to_string
fn char_to_string(c : Char) -> String {
  c.to_string()
}

#intrinsic("%fixedarray.join")
fn fixedarray_join(arr: FixedArray[String], sep: String) -> String {
  let mut result = ""
  for i = 0; i < arr.length(); i = i + 1 {
    if i > 0 {
      result += sep
    }
    result += arr[i]
  }
  result
}

fn init {
  println(char_to_string('a'))
  println(char_to_string('吉'))
  println(char_to_string('𠮷'))
  println(fixedarray_join(["a", "b", "c"], ", ")) // "a, b, c"
}
