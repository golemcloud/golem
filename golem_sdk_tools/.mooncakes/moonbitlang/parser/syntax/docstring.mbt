///|
pub(all) struct DocString {
  /// The docstring content.
  content : @list.List[String]
  /// The location of the docstring in the source code.
  loc : Location
}

///|
pub impl ToJson for DocString with to_json(self) {
  let mut docs = self.content
  let init_str = match docs {
    Empty => return ""
    More(head, tail~) => {
      docs = tail
      head
    }
  }
  let buf = StringBuilder::new()
  buf.write_string(init_str)
  for doc in docs {
    buf.write_string("\n")
    buf.write_string(doc)
  } nobreak {
    Json::string(buf.to_string())
  }
}

///|
pub fn DocString::empty() -> DocString {
  let dummy_pos = Position::{ lnum: 0, cnum: 0, bol: 0, fname: "" }
  { content: @list.empty(), loc: { start: dummy_pos, end: dummy_pos } }
}
