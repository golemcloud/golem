///|
fn mk_loc(loc : (Position, Position)) -> Location {
  Location::{ start: loc.0, end: loc.1 }
}
