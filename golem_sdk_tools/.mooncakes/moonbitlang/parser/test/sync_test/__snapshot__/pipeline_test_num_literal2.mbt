
fn init {
  println(2023)
  println(99)

  println(2147483648910L)
  println(21474836487654L)
}

fn match_i64(i: Int64) -> Unit {
  match i {
   9223372036854775807L => { println("MAX") }
   9223372036854775806L => { println("MAX-1") }
   -9223372036854775808L => { println("MIN") }
   -9223372036854775807L => { println("MIN+1") }
   _ => { println(i) }
  }
}

fn init {
  let max_i64 = 9223372036854775807L
  let min_i64 = -9223372036854775808L
  println(max_i64)
  println(min_i64)

  match_i64(max_i64)
  match_i64(max_i64 - 1L)
  match_i64(min_i64)
  match_i64(min_i64 + 1L)
  match_i64(911L)
 
  println(0L)
}
