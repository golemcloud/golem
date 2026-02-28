#alias(Renderable)
trait Render {
  render(Self) -> String
  render_with(Self, prefix : String) -> String = _
}

impl Renderable with render_with(self, prefix) {
  prefix + self.render()
}

impl Renderable for Int with render(self) {
  self.to_string()
}

struct Box[T] {
  value : T
}

impl[T : Renderable] Renderable for Box[T] with render(self) {
  "Box(" + self.value.render() + ")"
}

type Pair[T] = (T, T)
type RenderObj = &Renderable

fn[T : Renderable] use_pair(p : Pair[T]) -> String {
  let (a, b) = p
  a.render() + "," + b.render()
}

fn init {
  let p : Pair[Int] = (1, 2)
  let _ = use_pair(p)

  let b = Box::{ value: 3 }
  let obj : RenderObj = b
  let _ = obj.render_with("v=")

  let _ = Renderable::render(4)
  ()
}
