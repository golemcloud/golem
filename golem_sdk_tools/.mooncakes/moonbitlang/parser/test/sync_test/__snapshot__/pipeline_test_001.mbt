fn init {
  printX()
  println(a)
  nested()()
}

let a = 3

let data1 = "hello world\n"
let data2 = "\nbest xx"

fn printX() -> Unit {
  println(data1)
  println(data2)
}
fn nested () -> () -> Unit  {
  let hi = "hi"
  fn printY() {
    println(hi)
  }
  printY
}

struct MyArray(Array[Int]) derive(Show)

fn init {
  let v = [1,2,3]  
  let v : MyArray = [1,2, .. v]
  println(v)
}
