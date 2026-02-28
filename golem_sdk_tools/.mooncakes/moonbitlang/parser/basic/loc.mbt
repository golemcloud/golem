///|
pub(all) struct Position {
  fname : String
  lnum : Int
  bol : Int
  cnum : Int
} derive(Hash, ToJson)

///|
pub impl Show for Position with output(self, logger) {
  logger.write_char('"')
  if self.fname != "" {
    logger..write_string(self.fname).write_char(':')
  }
  logger
  ..write_string(self.lnum.to_string())
  ..write_char(':')
  ..write_string(self.column().to_string())
  .write_char('"')
}

///|
pub impl Eq for Position with equal(self, other) {
  self.fname == other.fname && self.cnum == other.cnum
}

///|
pub impl Compare for Position with compare(self, other) {
  match self.fname.compare(other.fname) {
    0 =>
      match self.lnum.compare(other.lnum) {
        0 => self.column().compare(other.column())
        r => r
      }
    r => r
  }
}

///|
test "compre position" {
  let a = { fname: "", lnum: 10, bol: 0, cnum: 0 }
  let b = { fname: "", lnum: 10, bol: 0, cnum: 0 }
  inspect(a.compare(b), content="0")
  let a = { fname: "", lnum: 10, bol: 0, cnum: 0 }
  let b = { fname: "", lnum: 1, bol: 0, cnum: 0 }
  inspect(a.compare(b), content="1")
  inspect(b.compare(a), content="-1")
  let a = { fname: "", lnum: 10, bol: 2, cnum: 5 }
  let b = { fname: "", lnum: 10, bol: 2, cnum: 5 }
  inspect(a.compare(b), content="0")
  let a = { fname: "", lnum: 10, bol: 2, cnum: 5 }
  let b = { fname: "", lnum: 10, bol: 2, cnum: 10 }
  inspect(a.compare(b), content="-1")
  inspect(b.compare(a), content="1")
}

///|
pub fn Position::column(self : Position) -> Int {
  self.cnum - self.bol + 1
}

///|
pub(all) struct Location {
  start : Position
  end : Position
} derive(Eq, Compare, Hash)

///|
pub fn Location::trim_first_char(self : Location) -> Location {
  Location::{
    start: Position::{
      fname: self.start.fname,
      lnum: self.start.lnum,
      bol: self.start.bol,
      cnum: self.start.cnum + 1,
    },
    end: self.end,
  }
}

///|
pub fn Location::trim_last_char(self : Location) -> Location {
  Location::{
    start: self.start,
    end: Position::{
      fname: self.end.fname,
      lnum: self.end.lnum,
      bol: self.end.bol,
      cnum: self.end.cnum - 1,
    },
  }
}

///|
pub impl Show for Location with output(self, buf) {
  buf
  ..write_string(self.start.fname)
  ..write_string("-")
  ..write_string(self.start.lnum.to_string())
  ..write_string(":")
  ..write_string(self.start.column().to_string())
  ..write_string("-")
  ..write_string(self.end.lnum.to_string())
  ..write_string(":")
  .write_string(self.end.column().to_string())
}

///|
pub impl ToJson for Location with to_json(loc) {
  match show_loc.val {
    Hidden => Json::null()
    Json =>
      {
        "file": loc.start.fname,
        "start": { "line": loc.start.lnum, "column": loc.start.column() },
        "end": { "line": loc.end.lnum, "column": loc.end.column() },
      }
    String => {
      let str = "\{loc.start.lnum}:\{loc.start.column()}-\{loc.end.lnum}:\{loc.end.column()}"
      Json::string(str)
    }
  }
}

///|
pub fn Location::merge(self : Location, other : Location) -> Location {
  guard self.start.fname == other.start.fname
  let start = if self.start < other.start { self.start } else { other.start }
  let end = if self.end > other.end { self.end } else { other.end }
  Location::{ start, end }
}
