trait Name {
  name(Self)->String
}

fn[N:Name] test_name(h:N)->String{
  let h_t = h as &Name
  h_t.name() // TODO: check this should auto-comlete
}

struct X(String)

impl Name for X with name(self) {
  self.0
}

fn init {
  println(test_name(X::X("h")))
}
