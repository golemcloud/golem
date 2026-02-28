///|
test "pretty group" {
  let doc = group(
    @prettyprinter.text("hello") +
    @prettyprinter.nest(@prettyprinter.hardline + @prettyprinter.text("world")),
  )
  inspect(
    doc.to_string(),
    content=(
      #|hello
      #|  world
    ),
  )
}
