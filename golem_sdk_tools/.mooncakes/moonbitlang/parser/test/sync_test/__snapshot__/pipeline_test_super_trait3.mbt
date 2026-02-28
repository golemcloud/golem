pub(all) struct Requirement(Int) derive(Show)

pub(all) enum Doc {
  Empty
  String(String)
  Blank(Int)
  IfFlat(Doc, Doc)
  HardLine
  Cat(Requirement, Doc, Doc)
  Nest(Requirement, Int, Doc)
  Group(Requirement, Doc)
  Align(Requirement, Doc)
  Custom(&Custom)
} derive(Show)

pub(open) trait Custom : Show  {
  requirement(Self) -> Requirement
  pretty(Self, Output, State, Int, Bool) -> Unit
}

pub(all) type Output
pub(all) type State

fn init {
  let a = Doc::Cat(Requirement(3), Empty,Empty)
  println(a)
}
