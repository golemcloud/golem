///|
pub(open) trait CodeGenerator {
  void_type(Self) -> CodeFragment
  void_action_code(Self) -> CodeFragment
  codegen_tokens(
    Self,
    Array[@grm.Terminal],
    (String) -> TerminalMeta,
    &Logger,
    no_comments~ : Bool,
    derive_map~ : @array_multimap.T[String, String],
  ) -> Unit
  codegen(
    Self,
    @grm.Grammar,
    @lr1.Automaton,
    MetaProvider,
    @logger_with_cursor.LoggerWithCursor,
    source_map_builder~ : &SourceMapBuilder?,
    grammar_filename~ : String,
    external_tokens~ : Bool,
    no_comments~ : Bool,
    mode~ : Mode,
    input_mode~ : InputMode,
  ) -> Unit
}
