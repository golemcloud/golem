fn output_string(n: String) -> Unit {
  for c in n {
    println(c)
  }
}

enum T {
  A
  B
}

let h:T = B

let x:(T, T) = (A, A)

let y:(T, T) = (h, h)

fn init {
  match (x, y) {
    ((A, A), (B, B)) => println("ok\n")
    _ => println("not ok\n")
  }
  match (x, y) {
    ((A, B), (B, B)) => println("not ok\n")
    _ => println("ok\n")
  }
  match (x, y) {
    ((A, _), (_, B)) => println("ok\n")
    (_, _) => println("not ok\n")
  }
  match (x, y) {
    ((A, _), (_, A)) => println("not ok\n")
    (_, _) => println("ok\n")
  }
  match (x, y) {
    ((A, _), (_, A)) => println("not ok\n")
    (p1, p2) =>
      match (p1, p2) {
        ((A, _), (B, B)) => println("ok\n")
        _ => println("not ok\n")
      }
  }
}

