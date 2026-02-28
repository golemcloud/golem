///|
pub type CharLiteral = String

///|
pub type StringLiteral = String

///|
pub type ByteLiteral = String

///|
pub type BytesLiteral = String

///|
pub(all) struct InterpSource {
  source : String
  loc : Location
} derive(Show, ToJson, Eq)

///|
pub(all) enum InterpElem {
  InterpLit(repr~ : String, loc~ : Location)
  InterpSource(InterpSource)
} derive(Show, Eq)

///|
pub type InterpLiteral = Array[InterpElem]

///|
fn InterpLiteral::to_string_repr(self : InterpLiteral) -> String {
  let buf = StringBuilder::new()
  buf.write_char('"')
  for elem in self {
    match elem {
      InterpLit(repr~, ..) => buf.write_string(repr)
      InterpSource(source) => {
        buf.write_string("\\{")
        buf.write_string(source.source)
        buf.write_char('}')
      }
    }
  } nobreak {
    buf.write_char('"')
    buf.to_string()
  }
}
