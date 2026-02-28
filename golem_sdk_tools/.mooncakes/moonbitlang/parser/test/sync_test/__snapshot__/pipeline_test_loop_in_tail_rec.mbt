struct Matrix {
  data : FixedArray[Int]
  w : Int
  h : Int
}

fn Matrix::sum_from(self : Matrix, i : Int, acc~ : Int = 0) -> Int {
  if i >= self.h { return acc }
  for
    j = 0, sum = acc
    j < self.w
    j = j + 1, sum = sum + self.data[i * self.w + j] {
  } else {
    self.sum_from(i + 1, acc=sum)
  }
}

fn init {
  let mat = {
    data: [ 1, 2, 3, 4, 5, 6, 7, 8, 9 ],
    w: 3,
    h: 3
  }
  mat.sum_from(1) |> println
}
