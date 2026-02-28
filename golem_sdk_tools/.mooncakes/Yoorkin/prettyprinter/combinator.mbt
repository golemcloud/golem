///|
/// Empty document.
/// `render(empty) == ""`
pub let empty : Document = Empty

///|
/// Force line break with current indentation.
/// This document will always doesn't fit in the current line.
/// 
/// # Example
/// 
/// ```
/// render(text("hello") + nest(hardline + text("world")))
/// |> inspect(
///   content=(
///     #|hello
///     #|  world
///   ),
/// )
/// ```
/// 
pub let hardline : Document = Line

///|
/// Space character.
/// `render(space) == " "`
pub let space : Document = Space

///|
/// Represents a line break if the line is too long, or nothing if it fits.
pub let softline : Document = switch(empty, line)

///|
/// Represents a line break if the line is too long, or a space if it fits.
pub let line : Document = switch(space, hardline)

///|
/// Ensures at least one line at this position.
/// ```
/// inspect(
///   mandatory_line + nest(line + text("abc")),
///   content=(
///     #|
///     #|  abc
///   ),
/// )
/// inspect(
///   mandatory_line + nest(text("abc")),
///   content=(
///     #|
///     #|abc
///   ),
/// )
/// ```
pub let mandatory_line : Document = MandatoryLine

///|
/// Inserts at least one space between concatenated text segments.
/// ```
/// inspect(
///   text("f();") + mandatory_space + text("//todo"),
///   content="f(); //todo",
/// )
/// inspect(
///   text("f();") + space + mandatory_space + text("//todo"),
///   content="f(); //todo",
/// )
/// inspect(
///   text("f();") + hardline + mandatory_space + text("//todo"),
///   content=(
///     #|f();
///     #|//todo
///   ),
/// )
/// ```
pub let mandatory_space : Document = MandatorySpace

///|
pub fn text(s : String) -> Document {
  Text(s)
}

///|
pub fn char(c : Char) -> Document {
  Text(c.to_string())
}

///|
/// Concatenate documents.
pub fn list(ls : Array[Document]) -> Document {
  ls.fold(init=Empty, concat)
}

///|
/// `group(doc)` tries to render the `doc` on a single line, 
/// but if the `doc` doesn't fit, or contains a `hardline`, all the `switch` 
/// inside the `doc` will be rendered with the right document.
pub fn group(doc : Document) -> Document {
  Group(doc.requirement(), doc)
}

///|
/// Switch between two documents based on the available space.
/// The left document represents the flatten mode, and the right one the pretty mode.
/// If the left document fits, it will be rendered, otherwise the right one.
pub fn switch(l : Document, r : Document) -> Document {
  Switch(l.requirement(), l, r)
}

///|
/// Nest the document by `n` spaces.
/// Any line breaks inside the nested document will be indented by `n` spaces.
pub fn nest(indent? : Int = 2, doc : Document) -> Document {
  Nest(doc.requirement(), indent, doc)
}

///|
/// Concatenate two documents.
/// You can also use the `+` operator.
pub fn concat(l : Document, r : Document) -> Document {
  Concat(l.requirement() + r.requirement(), l, r)
}

///|
/// This function takes a separator document, an array of values, and a mapping function, and returns a document
/// where each value in the array is mapped to a document using the mapping function, and then separated by the separator document.
///
/// # Arguments
///
/// * `sep` - The separator document to be used between each mapped value.
/// * `xs` - The array of values to be mapped and separated.
/// * `f` - The mapping function that takes a value from the array and returns a document.
///
/// # Returns
///
/// The resulting document where each value is mapped and separated by the separator document.
///
/// # Example
///
/// ```
/// let doc = separate_map(text(","), [1,2,3], pretty)
/// inspect(doc, content="1,2,3") 
/// ```
pub fn[A] separate_map(
  sep : Document,
  elems : Array[A],
  f : (A) -> Document,
) -> Document {
  separate(sep, elems.map(f))
}

///|
/// This function takes a separator document `sep` and an array of documents `docs` and returns a document
/// where each document in the array is separated by the separator document.
///
/// # Example
///
/// ```
/// let sep = text("-")
/// let docs = [text("Hello"), text("World"), text("!")]
/// separate(sep, docs).pretty() |> inspect(content="Hello-World-!")
/// ```
pub fn separate(sep : Document, docs : Array[Document]) -> Document {
  let cat_by_sep = fn(l, r) { concat(l, concat(sep, r)) }
  match docs {
    [] => empty
    [x, .. xs] => xs.iter().fold(init=x, cat_by_sep)
  }
}

///|
/// Surrounds the given document with the left and right documents.
/// 
/// `surround(l, r, doc)` is equivalent to `l + doc + r`
/// 
/// # Example
/// 
/// ```
/// inspect(surround(char('['), char(']'), text("doc")), content="[doc]")
/// ``` 
pub fn surround(left : Document, right : Document, doc : Document) -> Document {
  left + doc + right
}

///|
/// wraps the given document in parentheses.  
/// 
/// ```
/// inspect(parens(text("doc")), content="(doc)")
/// ```
pub fn parens(doc : Document) -> Document {
  surround(char('('), char(')'), doc)
}

///|
/// wraps the given document in brackets.  
/// 
/// ```
/// inspect(brackets(text("doc")), content="[doc]")
/// ```
pub fn brackets(doc : Document) -> Document {
  surround(char('['), char(']'), doc)
}

///|
/// wraps the given document in braces.
/// 
/// ```
/// inspect(braces(text("doc")), content="{doc}")
/// ```
pub fn braces(doc : Document) -> Document {
  surround(char('{'), char('}'), doc)
}

///|
/// print the string with double quotes.
/// 
/// ```
/// inspect(string("Hello, World!"), content="\"Hello, World!\"")
/// ```
pub fn string(str : String) -> Document {
  surround(char('"'), char('"'), text(str))
}

///|
/// print each documents in the same line if it fits, insert a line break otherwise.
/// 
/// ```
/// let words = [
///   "The", "quick", "brown", "fox", "jumps", "over", "the", "lazy", "dog",
/// ]
/// let doc = words.map(fn(x){text(x) + space}) |> flow()
/// inspect(doc, content="The quick brown fox jumps over the lazy dog ")
/// inspect(
///   render(doc, width=10),
///   content=(
///     #|The quick brown 
///     #|fox jumps over 
///     #|the lazy dog 
///   ),
/// )
/// ```
pub fn flow(doc : Array[Document]) -> Document {
  let sep = group(softline)
  doc.iter().map(pretty).intersperse(sep).fold(init=empty, concat)
}

///|
pub fn dynamic(f : (Context) -> Document) -> Document {
  let req = f({ indent: 0, width: 80, next_indent: 0 }).requirement() //FIXME
  Dynamic(req, f)
}
