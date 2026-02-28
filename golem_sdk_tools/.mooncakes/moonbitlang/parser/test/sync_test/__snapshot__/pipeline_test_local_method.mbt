test {
  inspect(1, content="1")
}

fn String::len(self : String) -> Int {
  self.length()
}

// in conflict
// fn len(self : Bytes) -> Int {
//   self.length()
// }


///| 
///
/// 
fn main{
  // String::len = self.length()  
  println("hello".len())
  println(String::len("hello"))
  // println(b"xx".len())
}
