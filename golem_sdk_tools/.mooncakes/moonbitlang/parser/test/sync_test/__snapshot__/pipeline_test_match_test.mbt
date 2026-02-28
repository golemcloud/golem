// TODO: support Lstringswitch
fn init {
  let x = 'c'
  match x {
    'a' => println('a')
    'b' => println('b')
    'c' => println('c')
    _ => println('d')
  }
  let x = 1.1
  match x {
    0.1 => println('a')
    1.1 => println('b')
    1.11 => println('c')
    _ => println('d')
  }
  let x = "ss"
  match x {
    "s" => println(1)
    "ss" => println(2)
    "sss" => println(3)
    _ => println(4)
  }
}

