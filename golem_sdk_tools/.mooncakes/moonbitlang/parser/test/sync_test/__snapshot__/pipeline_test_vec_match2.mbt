fn vv1(vv: Array[Array[Int]]) -> Int {
  match vv {
    [[a]] => a + 1
    [[a, b, .._]] => a + b + 2
    [[a, b, .._], [c, d, .._], .._] => a + b + c + d + 3
    [.. _, [.. _, a, b], [c, d, .._], [e]] => a + b + c + d + e + 4
    _ => 0
  }
}

fn tvv1(vv: Array[Array[Int]], exp: Int) -> Unit {
  if vv1(vv) == exp {
    println(".")
  } else {
    abort("error!")
  }
}

fn init {
  let v1 = [[100]]
  tvv1(v1, 101)
  let v2 = [[200, 300, 400]]
  tvv1(v2, 502)
  let v3 = [[200, 300, 400], [500, 600, 700]]
  tvv1(v3, 1603)
  let v4 = [[], [10, 20, 30], [30, 20, 60], [70]]
  tvv1(v4, 174)
  let v5 = []
  tvv1(v5, 0)
}
