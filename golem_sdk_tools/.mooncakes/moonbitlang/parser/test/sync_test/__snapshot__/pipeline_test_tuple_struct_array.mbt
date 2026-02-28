struct MyArray(Array[Int])

fn init {
  let a : MyArray = [ 1, 2, 3 ]
  a.0.each(println)
  a.0.iter().each(println)
  match a.0 {
    [ x ] => println(x)
    [ .. arr ] => println(arr)
  }
}

struct MyFixed(FixedArray[Int])

fn init {
  let a : MyFixed = [ 1, 2, 3 ]
  a.0.each(println)
  a.0.iter().each(println)
  match a.0 {
    [ x, y, z ] => println([ x, y, z ])
    _ => ()
  }
}
