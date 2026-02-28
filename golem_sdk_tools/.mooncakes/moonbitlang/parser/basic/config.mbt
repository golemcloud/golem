///|
pub let builtin_package : String = "moonbitlang/core/builtin"

///|
pub let current_package : Ref[String] = @ref.new("")

///|
/// Determines the location style when serializing the AST into JSON.
pub let show_loc : Ref[LocStyle] = @ref.new(Hidden)

///|
pub(all) enum LocStyle {
  Hidden
  /// show full location info
  Json
  /// uses a short string without the file name
  String
}
