fn init {
  let (a, b, c, d) = (1, 3.14, 'c', "long_string")
  println(a)

  println(b)

  println(c)

  println(d)

  let f = fn () { println("second_string\n") }
  let g = fn () { println("first_string\n") }
  g()
  f()
}

fn init {
  let arr: FixedArray[_] = [2, 0, 2, 3, 0, 3, 2, 8]
  let str = "hello_world!"
  println(arr[7])
  println(" = 8\n")
  println(arr[0] + arr.length())
  println(" = 10\n")
  println(str.get_char(arr[3]).unwrap())
  println(" = l\n")
  let f = fn (i) { arr[i] }
  let g = fn (i) { str.get_char(i).unwrap() }
  println(g(f(4)))
  println(" = h\n")
}

