trait XShow {
  repr(Self)->String
  to_string(Self)->String = _ // to_string -> to_stringx works
}

impl XShow with to_string(self){self.repr()+"!\n"} 


impl XShow for Int with repr(self) {
  self.to_string()
}
fn[X:XShow] use_xshow(x:X)->Unit{
  x.to_string()|>println
}
fn init {
  XShow::to_string(3)|>println
  // here (XShow,Int,to_string)
  // does not use the default implementation
  // since Int::to_string works
  use_xshow(3)
}

struct MyInt(Int)
impl XShow for MyInt with repr(self : MyInt) -> String {
  self.0.to_string()
}

fn init {
  XShow::to_string(MyInt::MyInt(3))|>println 
  // weird error message when typign MyInt(3)
  use_xshow(MyInt::MyInt(3))
}
