fn init {
  for i in 1..=2 {
    println(i)
  }
  for i in 1..<2 {
    println(i)
  }
  // iter range operator with side effect
  for i in { println("side effect test 1 =============================="); 1 }..<{println("side effect test 2 =============================="); 10} {
    println("side effect test \{i} ==============================")
  }
  for _ in 1..<3 {
    println("loop")
  }
  let arr = [1,2,3]
  for i in 0..<arr.length(){
    println(arr[i])
  }
  reversed_iter_range()
}

fn reversed_iter_range() -> Unit {
  let arr = [1,2,3]
  for i in 3>..0 {
    println(arr[i])
  }
  for i in 2>=..0 {
    println(arr[i])
  }
}

