struct Empty_record {

}

fn hello(x: Empty_record) -> Unit {
  let h : Empty_record = Empty_record::{  }
  match h {
    {} => println("ok\n")
  }
  let y : _? = None
  match (x, y) {
    ({}, Some(x)) => println("not ok")
    (_, None) => println("ok\n")
  }
}

fn init {
  hello(Empty_record::{  })
}

