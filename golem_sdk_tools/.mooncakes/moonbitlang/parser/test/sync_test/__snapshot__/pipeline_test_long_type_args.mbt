fn main {
  let large_tuple = (1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16)
  let large_tuple2 = (large_tuple, large_tuple, large_tuple, large_tuple)
  println((large_tuple2, large_tuple2, large_tuple2, 42))
  println((large_tuple2, large_tuple2, large_tuple2, "42"))
}
