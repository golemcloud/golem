
pub fn sum(n : Int) -> Int {
  fn taill_call_sum(n : Int, acc : Int) -> Int {
    if n == 0 {
      acc
    } else {
      taill_call_sum(n - 1, acc + n)
    }
  }

  taill_call_sum(n, 0)
}

pub fn sum2(n : Int) -> Int {
  fn taill_call_sum(n : Int, acc : Int) -> Int raise {
    if n == 0 {
      acc
    } else {
      taill_call_sum(n - 1, acc + n)
    }
  }

  try {
    taill_call_sum(n, 0)
  } catch {
    _ => abort("error")
  } else {
    x => x
  }
}

fn main {
// pub fn main wrong error message
  println(sum(10))
  println(sum2(10))
}
