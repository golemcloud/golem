///|
enum Document {
  Empty
  Line
  Space
  MandatoryLine
  MandatorySpace
  Text(String)
  Concat(Requirement, Document, Document)
  Group(Requirement, Document)
  Switch(Requirement, Document, Document)
  Nest(Requirement, Int, Document)
  Dynamic(Requirement, (Context) -> Document)
}

///|
pub struct Context {
  indent : Int
  next_indent : Int
  width : Int
} derive(Show)

///|
pub fn Context::current_remain(self : Self) -> Requirement {
  Space(@cmp.maximum(0, self.width - self.indent))
}

///|
pub fn Context::next_remain(self : Self) -> Requirement {
  Space(@cmp.maximum(0, self.width - self.next_indent))
}

///|
priv struct State {
  indent : Int
  fit : Bool
  doc : Document
}

///|
fn render_document(width : Int, doc : Document) -> String {
  let buf = StringBuilder::new()
  let stack = [{ indent: 0, fit: false, doc }]
  let mut column = 0
  // record the MandatoryLine that has not yet been decided to print
  let mut dangling_newline : Int? = None
  // record the MandatorySpace that has not yet been decided to print
  let mut dangling_space : Bool = false
  let mut has_leading_space : Bool = false
  let mut has_leading_newline : Bool = false
  fn write_newline(indent) {
    buf.write_char('\n')
    for _ in 0..<indent {
      buf.write_char(' ')
    }
  }

  while stack.pop() is Some({ indent, fit, doc }) {
    match doc {
      Empty => ()
      Line => {
        has_leading_newline = true
        write_newline(indent)
        column = indent
      }
      Space => {
        has_leading_space = true
        buf.write_char(' ')
        column += 1
      }
      Text(s) => {
        if dangling_newline is Some(indent) && !has_leading_newline {
          write_newline(indent)
        }
        if dangling_space && !has_leading_space && !has_leading_newline {
          buf.write_char(' ')
        }
        has_leading_space = false
        has_leading_newline = false
        dangling_space = false
        dangling_newline = None
        buf.write_string(s)
        column += s.length()
      }
      Concat(_, l, r) =>
        stack..push({ indent, fit, doc: r })..push({ indent, fit, doc: l })
      Nest(_, i, d) => stack..push({ indent: indent + i, fit, doc: d })
      Switch(_, a, b) =>
        stack.push(
          if fit {
            { indent, fit, doc: a }
          } else {
            { indent, fit, doc: b }
          },
        )
      Group(req, d) => {
        let fit = fit || Requirement::Space(column) + req <= Space(width)
        stack.push({ indent, fit, doc: d })
      }
      MandatoryLine => dangling_newline = Some(indent)
      MandatorySpace => dangling_space = true
      Dynamic(_, f) => {
        let doc = f({ indent: column, width, next_indent: indent })
        stack.push({ indent, fit, doc })
      }
    }
  }
  buf.to_string()
}

///|
pub impl Show for Document with output(self, buf) {
  buf.write_string(render_document(80, self))
}

///|
pub impl Add for Document with add(self, other) {
  concat(self, other)
}
