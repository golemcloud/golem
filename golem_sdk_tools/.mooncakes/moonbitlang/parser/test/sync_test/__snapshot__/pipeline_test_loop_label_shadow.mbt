//!build: -w -36
fn f(xs: Array[Int]) -> String {
  l~: for i in xs {
    let n =
    l~: loop (i, i) {
      (1, _) => { 
        println("break1")
        break l~ 42
      }
      (2, _) => {
        println("continue1")
        continue l~ (1, 2)
      }
      (a, b) => a + b
    }
    if i * 2 == n {
      println("continue2")
      continue l~
    } else {
      println("break2")
      break l~ "ok"
    }
  } else {
    "nothing"
  }
}

fn main {
  println(f([1, 2, 3]))
  println(f([2]))
  println(f([3]))
  println(f([]))

}
