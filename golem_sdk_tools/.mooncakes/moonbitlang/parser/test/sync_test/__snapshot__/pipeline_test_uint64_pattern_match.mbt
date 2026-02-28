

fn pop_count(c : UInt64) -> Int { 
   loop (0, c) { 
     (x, 0) => x 
     (c, w) => continue (c + 1, w & (w - 1)) 
   } 
 } 


fn main {
  // test pop_count
  println("===== testing pop_count:")
  println(pop_count(64))
 }