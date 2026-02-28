fn init {
  let a = 1..=10
  let b = 1..<10
  let c = 1L..=10L
  let c = 1L..=10
  let c = 1L..<10L
  let c = 1L..<10
  let c = 1U..=10U
  let c = 1U..=10
  let c = 1U..<10U
  let c = 1U..<10
  let c = 1UL..=10UL
  let c = 1UL..=10
  let c = 1UL..<10UL
  let c = 1UL..<10
  let c = 1.0..=10.0
  let c = (1.0 + 2.0) ..= (10.0 + 2.0)

  for i in c {
    println(i)
  }
  for i in 1..=2 {
    println(i)
  }
  for i in 1.0..=2 {
    println(i)
  }
  for i in a..<=b {
    println(i)
  }
  for i in a>..b {
    println(i)
  }
  for i in a>=..b {
    println(i)
  }
}
