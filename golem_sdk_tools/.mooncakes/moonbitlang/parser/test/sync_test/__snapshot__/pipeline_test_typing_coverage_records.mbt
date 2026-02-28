struct Point {
  x : Int
  y : Int
  tag : String
}

struct TaggedPoint {
  point : Point
  label : String?
}

fn shift(p : Point, dx : Int, dy : Int) -> Point {
  Point::{ ..p, x: p.x + dx, y: p.y + dy }
}

fn describe(t : TaggedPoint) -> String {
  let { point, label } = t
  let { x, y, tag } = point
  match label {
    Some(l) => l + ":" + tag + ":" + x.to_string() + "," + y.to_string()
    None => tag + ":" + x.to_string() + "," + y.to_string()
  }
}

fn tweak(t : TaggedPoint) -> TaggedPoint {
  match t {
    { point: { x, y, tag }, label: Some(label) } =>
      TaggedPoint::{
        point: Point::{ x: x + 1, y: y + 1, tag: tag },
        label: Some(label + "!"),
      }
    { point: { x, y, tag }, label: None } =>
      TaggedPoint::{ point: Point::{ x: x, y: y, tag: tag }, label: None }
  }
}

fn init {
  let p = Point::{ x: 1, y: 2, tag: "origin" }
  let moved = shift(p, 3, 4)
  let t1 = TaggedPoint::{ point: moved, label: Some("p") }
  let t2 = TaggedPoint::{ point: p, label: None }
  let _ = describe(t1)
  let _ = describe(t2)
  let _ = tweak(t1)
  let _ = tweak(t2)
  ()
}
