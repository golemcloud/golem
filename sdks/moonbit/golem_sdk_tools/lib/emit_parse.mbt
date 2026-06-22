// Shared helper for the string-template emitters: parse generated MoonBit
// source back into an `@syntax.Impl` AST and **fail loud** if the parser
// reports anything. A parse report on generated source always means the
// emitter produced malformed code — silently returning a partial AST would
// hide the bug and emit broken user files.

///|
/// Parses generated MoonBit source into impl AST nodes, aborting with the
/// reports + the offending source if the parser emits any diagnostics.
/// `context` labels the emitter (e.g. `"value types"`) in the abort message.
fn parse_generated_impls(
  source : String,
  context : String,
) -> @list.List[@syntax.Impl] {
  // Guard against MoonBit string interpolation in generated code: the formatter
  // (0.1.5) silently DROPS `\{...}` segments when round-tripping, corrupting the
  // emitted file in a way the parser cannot catch (the result still parses).
  // Generated code must use string concatenation (`"..." + x.to_string()`).
  if source.contains("\\{") {
    abort(
      "golem_sdk_tools: generated \{context} code contains a `\\{...}` string interpolation, which the formatter drops. Use string concatenation instead.\n--- generated source ---\n" +
      source,
    )
  }
  let (parsed, reports) = @parser.parse_string(source)
  if reports.length() > 0 {
    let buf = StringBuilder::new()
    buf.write_string(
      "golem_sdk_tools: generated \{context} code failed to parse:\n",
    )
    for r in reports {
      buf.write_string("  - \{r}\n")
    }
    buf.write_string("--- generated source ---\n")
    buf.write_string(source)
    abort(buf.to_string())
  }
  parsed
}
