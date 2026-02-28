///|
pub(all) struct Report {
  loc : Location
  msg : String
} derive(Show)

///|
pub impl Show for Report with to_string(self) {
  "error (\{self.loc}): \{self.msg}"
}

///|
pub impl ToJson for Report with to_json(self) {
  { "type": "Error", "loc": self.loc, "msg": self.msg }
}
