///|
pub fn f1(s : String) -> Unit {
  match s {
    ['c', ] => println(1)
    ['c', ..,] => println(1)
    ['c', .. b,] => println(1)
    ['c', .. b, 'c',] => println(1)
    _ => println(2)
  }
}

