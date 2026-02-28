fn init {
  ([ 1, 2, 3 ]: FixedArray[_]).each(println)
  println(([ [ 1, 2 ] ]: FixedArray[FixedArray[_]]) == [ [ 1, 2 ] ])
  println(([ [ 1, 2 ] ]: FixedArray[FixedArray[_]]) == [ [ 1, 3 ] ])
}
