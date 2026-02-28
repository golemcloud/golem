fn init {
  let a: FixedArray[_] = [1,2,3,4]
  let len = a.length()
  let sum = for i = 0, sum = 0 {
    if i < len {
      continue i + 1, sum + a[i]
    } else {
      break sum
    }
  }
  println(sum)

  let i = 1001
  let n = for {
    if i >= 999 * 1000 { break "999k+" }
    if i >= 1000 { break "1k" }
    break "\{i}"
  }
  println(n)

}
