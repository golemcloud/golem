///|
fn parse_integer(input : BytesView) -> Int {
  loop (0, input) {
    (acc, []) => acc
    (acc, ['0'..='9' as c, .. rest]) =>
      continue (acc * 10 + (c.to_int() - '0'), rest)
    (_, _) => panic()
  }
}

///|
fn eval(input : BytesView) -> Int? {
  let stack : Array[Int] = []
  for curr = input {
    lexmatch curr with longest {
      "" => break
      (" ", next) => continue next
      ("[0-9]+" as num, next) => {
        let value = parse_integer(num)
        stack.push(value)
        continue next
      }
      ("\+", next) =>
        if stack.pop() is Some(b) && stack.pop() is Some(a) {
          stack.push(a + b)
          continue next
        } else {
          return None
        }
      ("-", next) =>
        if stack.pop() is Some(b) && stack.pop() is Some(a) {
          stack.push(a - b)
          continue next
        } else {
          return None
        }
      _ => return None
    }
  }
  stack.pop()
}

///|
fn main {
  println(eval("42"))
  println(eval("3 2 +"))
  println(eval("3 2 + 5 -"))
  println(eval("5 3 2 + -"))
}
