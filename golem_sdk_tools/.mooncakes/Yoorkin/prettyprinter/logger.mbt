///|
/// Represent an enum value.
/// 
/// # Example
/// 
/// ```
/// let doc = ctor(pkg="pkg", ty="Enum", "Constr", [pretty(1), pretty(2)], labeled={
///   "loc": pretty(100),
///   "name": pretty("cons"),
/// })
/// inspect(
///   doc,
///   content=(
///     #|@pkg.Enum::Constr(1, 2, loc=100, name="cons")
///   ),
/// )
/// let doc = ctor(
///   pkg="pkg",
///   ty="Enum",
///   "LongConstructor",
///   [
///     pretty(100000000),
///     pretty(200000000),
///     pretty(100000000),
///     pretty(200000000),
///     pretty(100000000),
///     pretty(200000000),
///     pretty(100000000),
///     pretty(200000000),
///   ],
///   labeled={ "loc": pretty(100), "name": pretty("cons") },
/// )
/// inspect(
///   doc,
///   content=(
///     #|@pkg.Enum::LongConstructor(
///     #|  100000000,
///     #|  200000000,
///     #|  100000000,
///     #|  200000000,
///     #|  100000000,
///     #|  200000000,
///     #|  100000000,
///     #|  200000000,
///     #|  loc=100,
///     #|  name="cons",
///     #|)
///   ),
/// )
/// ```
pub fn ctor(
  pkg? : String,
  ty? : String,
  ctor : String,
  payloads : Array[Document],
  labeled? : Map[String, Document] = {},
) -> Document {
  let pkg = match pkg {
    None => empty
    Some(pkg) => char('@') + text(pkg) + char('.')
  }
  let ty = match ty {
    None => empty
    Some(ty) => text(ty + "::")
  }
  let ctor = text(ctor)
  let xs = []
  payloads.each(x => xs.push((None, x)))
  labeled.each((k, v) => xs.push((Some(k), v)))
  let xs = separate_map(char(',') + line, xs, fn(p) {
      match p.0 {
        None => p.1
        Some(name) => text(name) + text("=") + p.1
      }
    }) +
    switch(empty, char(','))
  group(pkg + ty + ctor + parens(nest(softline + xs) + softline))
}

///|
/// Represent a struct.
/// 
/// # Example
/// 
/// ```
/// let doc = record(pkg="pkg", ty="Struct", {
///   "spos": pretty(1),
///   "epos": pretty(2),
///   "file": pretty("abc"),
/// })
/// inspect(
///   doc,
///   content=(
///     #|@pkg.Struct::{spos: 1, epos: 2, file: "abc"}
///   ),
/// )
/// let doc = record(pkg="pkg", ty="Struct", {
///   "spos": pretty(1000000000),
///   "epos": pretty(2000000000),
///   "file": pretty("abc"),
///   "spos1": pretty(1000000000),
///   "epos1": pretty(2000000000),
///   "file1": pretty("abc"),
/// })
/// inspect(
///   doc,
///   content=(
///     #|@pkg.Struct::{
///     #|  spos: 1000000000,
///     #|  epos: 2000000000,
///     #|  file: "abc",
///     #|  spos1: 1000000000,
///     #|  epos1: 2000000000,
///     #|  file1: "abc",
///     #|}
///   ),
/// )
/// ```
pub fn record(
  pkg? : String,
  ty? : String,
  fields : Map[String, Document],
) -> Document {
  let pkg = match pkg {
    None => empty
    Some(pkg) => text("@" + pkg + ".")
  }
  let ty = match ty {
    None => empty
    Some(ty) => text(ty + "::")
  }
  let fields = separate_map(char(',') + line, fields.to_array(), fn(p) {
      text(p.0) + text(":") + space + p.1.pretty()
    }) +
    switch(empty, char(','))
  group(pkg + ty + braces(nest(softline + fields) + softline))
}

///|
/// Represent a tuple.
/// 
/// # Example
/// 
/// ```
/// let doc = tuple([pretty(1), pretty('a'), pretty(true)])
/// inspect(doc, content="(1, a, true)")
/// let elem = pretty("very long string")
/// let doc = tuple(Array::make(5, elem))
/// inspect(
///   doc,
///   content=(
///     #|(
///     #|  "very long string",
///     #|  "very long string",
///     #|  "very long string",
///     #|  "very long string",
///     #|  "very long string",
///     #|)
///   ),
/// )
/// ```
/// 
pub fn tuple(elems : Array[Document]) -> Document {
  let elems_doc = separate_map(char(',') + line, elems, fn(x) { x }) +
    switch(empty, char(','))
  group(parens(nest(softline + elems_doc) + softline))
}
