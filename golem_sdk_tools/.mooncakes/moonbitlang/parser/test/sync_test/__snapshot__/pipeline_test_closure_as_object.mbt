trait Any {}

let arr : Array[&Any] = []

fn[T] push_item(item : T) -> Unit {
  arr.push(item)
}

fn main {
  push_item(x => x + 1)
  push_item(x => println(x + "!"))
  println(arr.length())
}
