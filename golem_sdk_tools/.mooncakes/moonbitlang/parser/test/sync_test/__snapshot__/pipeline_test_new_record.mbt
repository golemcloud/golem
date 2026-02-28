struct Rect {
  w:Int
  h:Int
}

enum Color {
  Red (Int)
  Green
  Blue
}

fn area(r: Rect) -> Int {
  r.w * r.h
}

fn output_color(c: Color) -> Unit {
  match c {
    Red(i) => {
      let color = "RED"
      println(color)
      println(i)
    }
    Green => println("Green")
    Blue => println("Blue")
  }
}

fn init {
  println(area({ w:3, h:2 }))
  output_color(Red(3))
}

