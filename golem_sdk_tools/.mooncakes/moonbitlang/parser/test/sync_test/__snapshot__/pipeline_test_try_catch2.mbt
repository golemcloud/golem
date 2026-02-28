suberror IntError {
  IntError(Int)
}

fn find_index(x : Array[Int], y : Int) -> Int? {
  try {
    for i = 0; i < x.length() ; i = i + 1{
      if x[i] == y {
        raise IntError(i)
      }
    }
    None
  }catch {
    IntError(i) => Some(i)
  }
}

fn init {
  find_index([1,2,3], 2)|>println
  find_index([1,2,3,4], 4)|>println
  find_index([1,2,3,4], 0)|>println
}

fn find_index2(x : Array[Int], y : Int, z : Int) -> Unit {
  try {
    for i = 0; i < x.length() ; i = i + 1{
      if x[i] == y {
        raise IntError(i)
      } else if x[i] == z {
        raise IntError(i)
      } 
    }

  } catch {
    IntError(i) => println(i)
  }
}

fn init {
  find_index2([1,2,3], 2, 3)
  find_index2([1,2,3,4], 4, 3)
  find_index2([1,2,3,4], 0, 3)
}
