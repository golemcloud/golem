fn min(a: Int, b: Int) -> Int {
  if a < b { a } else { b }
}

fn print_int_endline(i: Int) -> Unit {
  println(i)
}

fn levenshtein(a: String, b: String) -> Int {
  let m = a.length()
  let n = b.length()
  let s = FixedArray::make(2, ([]: FixedArray[Int]))
  s[0] = FixedArray::make(n + 1, 0)
  s[1] = FixedArray::make(n + 1, 0)
  let mut j = 1
  while j <= n {
    s[0][j] = j
    j = j + 1
  }
  let mut i = 1
  while i <= m {
    // print_int_endline(obj_magic(s));
    s[i.land(1)][0] = i
    let mut j = 1
    while j <= n {
      s[i.land(1)][j] = if a[i - 1] == b[j - 1] {
        s[(i - 1).land(1)][j - 1]
      } else {
        1 +
        min(
          min(s[(i - 1).land(1)][j - 1], s[(i - 1).land(1)][j]),
          s[i.land(1)][j - 1],
        )
      }
      j = j + 1
    }
    i = i + 1
  }
  s[m.land(1)][n]
}

fn init {
  println(levenshtein("abcdheloworldabcd", "abcdhellowoorldabcd"))
}

