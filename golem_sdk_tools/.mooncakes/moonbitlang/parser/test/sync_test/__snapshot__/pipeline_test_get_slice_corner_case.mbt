fn init {
  println("===")
  let arr = [ 1, 2, 3 ]
  println({ println("evaluating arr"); arr }[1:])
}

fn init {
  println("===")
  let mut arr = [ 1, 2, 3 ]
  println(arr[ {arr=[]; 1} : ])
}

fn init {
  println("===")
  let arr = [ 1, 2, 3 ]
  {println("evaluating arr"); arr}[ {println("evaluating start"); 1} : {println("evaluating end"); 2} ]
  |> println
}

fn init {
  println("===")
  let arr = [ 1, 2, 3 ]
  arr[ {arr.push(4); 0}: ]
  |> println
}
