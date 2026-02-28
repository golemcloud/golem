


fn concat_string(ls : FixedArray[String]) -> String {
  let buffer = @buffer.new(size_hint=0)
  let mut i = 0
  while i < ls.length() {
    buffer.write_string(ls[i])
    i = i + 1
  }
  buffer.contents().to_unchecked_string()
}


fn concat_char(ls : FixedArray[Char]) -> String {
  let buffer = @buffer.new(size_hint=0)
  let mut i = 0
  while i < ls.length() {
    buffer.write_char(ls[i])
    i = i + 1
  }
  buffer.contents().to_unchecked_string()
}

fn init {
  println(concat_string(["123","eng eng eng eng eng","中文test中文test中文test","123123123123123123123123"]))
  println(concat_char(['东','n','∑','🍺']))
}
