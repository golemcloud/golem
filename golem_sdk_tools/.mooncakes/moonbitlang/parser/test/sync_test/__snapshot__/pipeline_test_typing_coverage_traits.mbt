trait Render {
  render(Self) -> String
  render_with(Self, prefix : String) -> String = _
}

impl Render with render_with(self, prefix) {
  prefix + self.render()
}

impl Render for Int with render(self) {
  self.to_string()
}

impl Render for String with render(self) {
  self
}

impl[T : Render, U : Render] Render for (T, U) with render(self) {
  "(" + self.0.render() + ", " + self.1.render() + ")"
}

type RObj = &Render

fn use_obj(r : RObj) -> String {
  r.render_with("val: ")
}

fn init {
  let a : RObj = (1 as RObj)
  let b : RObj = (("x", 2) as RObj)
  let _ = use_obj(a)
  let _ = use_obj(b)
  let _ = Render::render(3)
  let _ = Render::render_with(4, "n=")
  ()
}
