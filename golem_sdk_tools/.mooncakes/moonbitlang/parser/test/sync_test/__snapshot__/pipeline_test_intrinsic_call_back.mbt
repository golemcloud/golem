fn add(x : Int, y : Int) -> Int = "%i32_add"

fn id(x : (Int,Int) -> Int) -> ((Int,Int) -> Int)  {x}

// impl Add for Int with f(x, y) {x + y}

fn main {
  let _ =id(add)(1, 2)
  
}