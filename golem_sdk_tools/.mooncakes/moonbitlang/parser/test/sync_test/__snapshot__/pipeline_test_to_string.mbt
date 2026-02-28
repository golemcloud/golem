fn init {
  let printable = " !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~"
  for c in printable {
    println(c.to_string())
  }
  println('\n'.to_string())
}

fn init {
  let xs: FixedArray[_] = [0, 1, 12, 123, 1234, 12345, 123456, 1234567, 12345678, 123456789]
  let mut i = 0
  while i < xs.length() {
    println(xs[i].to_string())

    let y = xs[i].neg()
    println(y.to_string())

    i = i + 1
  }
}
