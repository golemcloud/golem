fn sum(arr: FixedArray[Int], i: Int, acc: Int) -> Int {
  if i < arr.length() { sum(arr, i + 1, acc + arr[i]) } else { acc }
}

let b:FixedArray[Int] = [1, 2, 3, 4, 88]

fn init {
  let mut i = 0
  while i < b.length() {
    println(b[i])
    i = i + 1
  }
  println(b.length())
  println(sum(b, 0, 0))
}

