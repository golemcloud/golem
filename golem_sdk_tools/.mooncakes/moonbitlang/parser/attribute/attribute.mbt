///|
pub(all) struct Id {
  qual : String?
  name : String
}

///|
pub impl Show for Id with output(self, logger) {
  match self.qual {
    None => logger.write_string(self.name)
    Some(x) =>
      logger..write_string(x)..write_string(".").write_string(self.name)
  }
}

///|
pub(all) enum Expr {
  Ident(Id)
  String(@tokens.StringLiteral)
  Apply(Id, List[Prop])
  Bool(Bool)
} derive(Show)

///|
pub(all) enum Prop {
  Labeled(String, Expr)
  Expr(Expr)
} derive(Show)

///|
pub(all) struct Attribute {
  loc : Location
  raw : String
  parsed : Expr?
  //mut used : Bool
} derive(Show)

///|
pub fn Attribute::name(self : Self) -> String? {
  match self.parsed {
    Some(Ident(id) | Apply(id, _)) => Some(id.name)
    _ => None
  }
}

///|
pub impl ToJson for Attribute with to_json(attr) {
  { "type": "Attribute", "raw": attr.raw, "loc": attr.loc }
}

///|
pub fn Attribute::new(
  loc~ : Location,
  content : (String, String?, String),
) -> Attribute {
  //TODO: report better syntax error for attribute 
  let (id, dot_id, raw_payload) = content
  let raw_payload = raw_payload.trim(char_set=" ").to_string()
  let (raw, attr_id) = match dot_id {
    None => ("#\{id}\{raw_payload}", { qual: None, name: id })
    Some(dot_id) => ("#\{id}.\{dot_id}", { qual: Some(id), name: dot_id })
  }
  let { errors, docstrings: _, tokens } = @lexer.tokens_from_string(
    raw_payload,
    comment=false,
    start_pos=loc.start,
  )
  let parsed = match errors {
    [] =>
      try payloads(tokens, initial_pos=loc.start) catch {
        UnexpectedEndOfInput(_) | UnexpectedToken(_) => Some(Ident(attr_id))
      } noraise {
        Empty => Some(Ident(attr_id))
        props => Some(Apply(attr_id, props))
      }
    _ => None
  }
  { loc, raw, parsed }
}
