//!build: -w -37

///| basic usage
fn f(xs : Array[Array[Int]], target : Int) -> (Int, Int) {
  outer~: for i in 0..<xs.length() {
    inner~: for j in 0..<xs[i].length() {
      if xs[i][j] == target {
        break outer~ (i, j)
      }
    }
  } else {
    (-1, -1)
  }
  
}

///|
fn g(x : Int?, y : Int?) -> (Int, Int) {
  l1~: loop (x, y) {
    (None, None) => continue (Some(10), Some(10))
    (Some(x), None) =>
      l2~:for i = 0, acc = 0; i < x; i = i + 1 {
        if i == 10 {
          break l1~ (i, acc)
        } else {
          continue l2~ i+2, acc+i
        }
      } else {
        (-1, -1)
      }
    (None, Some(y)) => {
      let mut i = 0
      let mut acc = 0
      l2~: while i < y {
        if i == 10 {
          break l2~ (i, acc)
        } else {
          i = i + 2
          acc = acc + i
          continue l2~
        }
      } else {
        (-2, -2)
      }
    }
    (Some(x), Some(y)) => {
      let mut step = 0
      l2~: for i = 0; i < x; i = i + 1 {
        l3~: for j = 0; j < y; j = j + 1 {
          step = step + 1
          if j == 10 {
            continue l2~ i + 2
          }
          if i + j == 15 {
            println("step: \{step}")
            break l1~ (i, j)
          }
        }
      } else {
        (-1, -1)
      }
    }
  }
}

///|
fn main {
  println(f([[1, 2, 3], [4, 5, 6], [7, 8, 9]], 5))
  println(f([[1, 2, 3], [4, 5, 6], [7, 8, 9]], 10))
  println(g(None, None))
  println(g(Some(7), Some(11)))
  println(g(Some(1), Some(1)))
  println(g(None, Some(11)))
  println(g(Some(11), None))
}
