fn init {
  let mut i = 0
  let mut sum = 0
  while true {
    i = i + 1
    sum = sum + i
    if i > 100 { break }
  }
  println(sum)

}

fn init {
  let mut i = 0
  let mut sum = 0
  while i < 10 {
    i = i +
    1
    // i % 2 == 0 should be supported
    if i == 6 || i == 5 { continue }
    sum = sum + i
  }
  println(sum)
}


