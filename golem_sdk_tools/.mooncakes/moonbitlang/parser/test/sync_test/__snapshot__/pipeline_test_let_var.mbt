let x1 = 0

let sum0: Ref[Int] = { val: 1 }
fn init {
  println(x1)
  println(sum0.val)
  println(sum1.val)
  println(sum2.val)
  println(sum3)
}

let sum1: Ref[Int] = { val: 2 }
let sum2: Ref[Int] = { val: 3 }
let sum3 = 3
