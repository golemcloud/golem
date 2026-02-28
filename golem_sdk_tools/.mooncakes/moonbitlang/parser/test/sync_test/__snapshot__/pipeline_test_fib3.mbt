fn fib(n : Int)->Int{
  for i = 0, a = 1 , b = 1 {
    if i >= n {
      break a  
    }
    continue i + 1, b , a + b
  }
}


fn init {
  fib(3)|>println
  fib2(3)|>println
}

fn fib2(n : Int)->Int{
  for i = 0, a = 1 , b = 1 {
    if i >= n {
      break a
    } else {
      continue i + 1, b , a + b
    }
  }
}

fn max(i: Int, n : Int)->Int{
  for {
    if i >= n {
      return i
    } else {
      return n
    }
  }
}

fn init {
  max(1, 2) |> println
  max(3, 2) |> println
}
