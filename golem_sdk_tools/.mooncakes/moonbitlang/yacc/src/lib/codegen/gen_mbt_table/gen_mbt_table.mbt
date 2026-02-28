// Copyright (C) 2025 International Digital Economy Academy
//
// This program is free software; you can redistribute it and/or
// modify it under the terms of the GNU General Public License
// as published by the Free Software Foundation; version 2.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program; if not, see <https://www.gnu.org/licenses/>.

///|
priv enum CodegenSymbol {
  T(@grm.Terminal)
  NT(@grm.Nonterminal)
  EOI
} derive(Eq, Compare)

///|
fn CodegenSymbol::eoi() -> CodegenSymbol {
  EOI
}

///|
fn CodegenSymbol::terminal(term : @grm.Terminal) -> CodegenSymbol {
  T(term)
}

///|
fn CodegenSymbol::nonterminal(nont : @grm.Nonterminal) -> CodegenSymbol {
  NT(nont)
}

///|
fn CodegenSymbol::encode(self : CodegenSymbol, grammar : @grm.Grammar) -> Int {
  match self {
    EOI => 0
    T(t) => 1 + t.num
    NT(nt) => 1 + grammar.terminals.length() + nt.num
  }
}

///|
priv enum CodegenDecision {
  Error
  Accept
  Shift(@lr1.LR1State)
  Reduce(@grm.Production)
  ReduceNoLookahead(@grm.Production)
} derive(Eq, Compare)

///|
fn derive_type_ident(type_ : @elab.TypeExpr) -> String {
  match type_ {
    Constr(pkg~, name, args) => {
      let primary = match pkg {
        None => name
        Some(pkg) => {
          let pkg2 = pkg.replace_all(old="/", new="_")
          "_\{pkg2}_\{name}"
        }
      }
      if args.length() == 0 {
        primary
      } else {
        primary + "_" + args.iter().map(derive_type_ident).join("__") + "_"
      }
    }
    Param(_) => panic()
    Option(type_) =>
      if type_ is Arrow(_, _) {
        "_" + derive_type_ident(type_) + "__"
      } else {
        derive_type_ident(type_) + "_"
      }
    Tuple(types) => "_" + types.iter().map(derive_type_ident).join("__") + "_"
    Arrow(args, ret) =>
      "_" +
      args.iter().map(derive_type_ident).join("__") +
      "_____" +
      derive_type_ident(ret)
  }
}

///|
const PUB_TYPE_TOKEN_KIND = "TokenKind"

///|
const PUB_TYPE_TOKEN = "Token"

///|
const PUB_TYPE_PARSE_ERROR = "ParseError"

///|
fn codegen_tokens(
  terminals : Array[@grm.Terminal],
  terminal_meta : (String) -> @codegen.TerminalMeta,
  output : &Logger,
  no_comments? : Bool = false,
  derive_map? : @array_multimap.T[String, String] = @array_multimap.new(),
) -> Unit {
  ignore(no_comments)
  fn inject_derive(type_ : String) -> @codegen.CodeFragment {
    match derive_map.get(type_) {
      [] => ""
      traits => {
        let joined = traits.iter().join(", ")
        " derive(\{joined})"
      }
    }
  }

  output.write_string(
    (
      $|pub(all) enum Token {
      $|
    ),
  )
  for term in terminals {
    let meta = terminal_meta(term.name)
    if meta.data_type is Constr(pkg=None, "Unit", []) {
      output.write_string(
        (
          $|  \{term.name}
          $|
        ),
      )
    } else {
      output.write_string(
        (
          $|  \{term.name}(\{meta.data_type})
          $|
        ),
      )
    }
  }
  output.write_string(
    (
      $|}\{inject_derive(PUB_TYPE_TOKEN)}
      $|
      $|pub fn Token::kind(self : Token) -> TokenKind {
      $|  match self {
      $|
    ),
  )
  for term in terminals {
    let meta = terminal_meta(term.name)
    if meta.data_type is Constr(pkg=None, "Unit", []) {
      output.write_string(
        (
          $|    \{term.name} => TK_\{term.name}
          $|
        ),
      )
    } else {
      output.write_string(
        (
          $|    \{term.name}(_) => TK_\{term.name}
          $|
        ),
      )
    }
  }
  output.write_string(
    (
      $|  }
      $|}
      $|
      $|
    ),
  )

  // TokenKind
  output.write_string(
    (
      $|pub(all) enum TokenKind {
      $|
    ),
  )
  for term in terminals {
    output.write_string(
      (
        $|  TK_\{term.name}
        $|
      ),
    )
  }
  output.write_string(
    (
      $|}\{inject_derive(PUB_TYPE_TOKEN_KIND)}
      $|
      $|
    ),
  )

  // TokenKind::to_string
  output.write_string(
    (
      $|pub impl Show for TokenKind with output(self, logger) {
      $|  logger.write_string(
      $|    match self {
      $|
    ),
  )
  for term in terminals {
    let name = match terminal_meta(term.name).image {
      Some(image) => image
      None => term.name
    }
    output.write_string(
      (
        $|      TK_\{term.name} => \{name.escape()}
        $|
      ),
    )
  }
  output.write_string(
    (
      $|    }
      $|  )
      $|}
      $|
      $|
    ),
  )
}

///|
fn codegen(
  grammar : @grm.Grammar,
  automaton : @lr1.Automaton,
  meta : @codegen.MetaProvider,
  output : @logger_with_cursor.LoggerWithCursor,
  source_map_builder? : &@codegen.SourceMapBuilder,
  grammar_filename~ : String,
  external_tokens? : Bool = false,
  no_comments? : Bool = false,
  mode? : @codegen.Mode = Default,
  input_mode? : @codegen.InputMode = Array,
  compress~ : Bool,
) -> Unit {
  let used_runtime_funcs = @sorted_set.new()
  let { terminal_meta, nonterminal_meta, production_meta, .. } = meta
  fn inject_derive(type_ : String) -> @codegen.CodeFragment {
    match meta.derive_map.get(type_) {
      [] => ""
      traits => {
        let joined = traits.iter().join(", ")
        " derive(\{joined})"
      }
    }
  }

  // Header
  for chunk in meta.header {
    let (code, original_range) = chunk
    match (source_map_builder, original_range) {
      (None, _) | (_, None) => ()
      (Some(source_map_builder), Some((original_utf8_pos, utf8_len))) => {
        let generated_utf8_pos = output.cursor()
        source_map_builder.add_mapping(
          grammar_filename, original_utf8_pos, generated_utf8_pos, utf8_len,
        )
      }
    }
    output.write_string(code)
  }

  // Position
  output.write_string(
    (
      $|
      $|
    ),
  )

  // Token, TokenKind
  if external_tokens {
    ()
  } else {
    codegen_tokens(
      grammar.terminals,
      terminal_meta,
      output,
      no_comments~,
      derive_map=meta.derive_map,
    )
  }

  // ParseError
  output.write_string(
    (
      $|pub suberror ParseError {
      $|  UnexpectedToken(Token, (\{meta.position_data_type}, \{meta.position_data_type}), Array[TokenKind])
      $|
    ),
  )
  match input_mode {
    Array =>
      output.write_string(
        (
          $|  UnexpectedEndOfInput(\{meta.position_data_type}, Array[TokenKind])
          $|
        ),
      )
    Pull => ()
  }
  output.write_string(
    (
      $|}\{inject_derive(PUB_TYPE_PARSE_ERROR)}
      $|
      $|
    ),
  )

  // YYObj_xxx
  output.write_string(
    (
      $|type YYObj = Error
      $|
      $|
    ),
  )
  match mode {
    Default => {
      let data_types = @sorted_set.new()
      for term in grammar.terminals {
        let meta = terminal_meta(term.name)
        if meta.data_type is Constr(pkg=None, "Unit", []) {
          ()
        } else {
          data_types.add(meta.data_type)
        }
      }
      let dedup : Set[@stamp.T] = Set::new()
      fn add_action(action : @elab.Action) {
        if dedup.add_and_check(action.stamp) {
          for sub_action in action.sub_actions {
            add_action(sub_action.action)
          }
          for binding in action.bindings {
            if binding.0 is Data(_, type_~) {
              if type_ is Constr(pkg=None, "Unit", []) {
                ()
              } else {
                data_types.add(type_)
              }
            }
          }
          data_types.add(action.type_)
        }
      }

      for state in automaton.states {
        for _, action in state.action {
          match action {
            Shift(_) => ()
            Accept => ()
            Reduce(production) =>
              if not(grammar.starts.contains(production)) {
                let meta = production_meta(production.num)
                add_action(meta.action)
              }
            Conflict(_) => panic()
          }
        }
      }
      output.write_string(
        (
          $|priv suberror YYObj_Void
          $|
          $|
        ),
      )
      for data_type in data_types {
        output.write_string(
          (
            $|priv suberror YYObj_\{derive_type_ident(data_type)} \{data_type}
            $|
            $|
          ),
        )
      }
    }
    JsonCst(_) => {
      if input_mode is Array {
        output.write_string(
          (
            $|priv suberror YYObj_Void
            $|
            $|
          ),
        )
      }
      output.write_string(
        (
          $|priv suberror YYObj_Json Json
          $|
          $|
        ),
      )
    }
  }

  // YYState, YYAction, YYDecision
  output.write_string(
    (
      $|type YYState = Int
      $|
      $|type YYAction = (\{meta.position_data_type}, ArrayView[(YYObj, \{meta.position_data_type}, \{meta.position_data_type})]) -> YYObj
      $|
      $|priv enum YYDecision {
      $|  Accept
      $|  Shift(YYState)
      $|  Reduce(Int, YYSymbol, YYAction)
      $|  ReduceNoLookahead(Int, YYSymbol, YYAction)
      $|  Error
      $|}
      $|
      $|
    ),
  )

  // YYSymbol
  output.write_string(
    (
      $|type YYSymbol = Int
      $|
      $|
    ),
  )

  // yy_action_xxx
  let actions : @hashmap2.T[@stamp.T, @elab.Action] = @hashmap2.new(capacity=16)
  fn add_action(action : @elab.Action) {
    if actions.contains(action.stamp) {
      return
    }
    for sub_action in action.sub_actions {
      add_action(sub_action.action)
    }
    actions.set(action.stamp, action)
  }

  for state in automaton.states {
    for _, decision in state.action {
      if decision is Reduce(production) {
        guard not(grammar.starts.contains(production)) else { continue }
        let meta = production_meta(production.num)
        add_action(meta.action)
      }
    }
  }
  let mut next_action_id = 0
  let stamp_to_action_id = @default_hashmap.new(fn(_key) {
    let action_id = next_action_id
    next_action_id += 1
    action_id
  })
  fn get_action_id(action : @elab.Action) -> Int {
    stamp_to_action_id.get(action.stamp)
  }

  for _, action in actions {
    let clause_info = action.original_clause_info
    output.write_string("// file:///./\{clause_info.file}\n")
    let clause_lines = clause_info.code.split("\n").to_array()
    let max_line_number = clause_info.line + 1 + (clause_lines.length() - 1)
    let num_digits = max_line_number.to_string().length()
    for index, line in clause_lines {
      output.write_string("// ")
      let line_number = clause_info.line + 1 + index
      let line_number_str = line_number.to_string().pad_start(num_digits, ' ')
      output.write_string("\{line_number_str}")
      output.write_char('|')
      if index == 0 {
        if clause_info.column > 0 {
          output.write_string(" ".repeat(clause_info.column))
        }
      }
      output.write_string(line.to_string())
      output.write_char('\n')
    }
    output.write_string(
      (
        $|fn yy_action_\{get_action_id(action)}(_last_pos : \{meta.position_data_type}, _args : ArrayView[(YYObj, \{meta.position_data_type}, \{meta.position_data_type})]) -> YYObj {
        $|
      ),
    )
    for index, sub_action in action.sub_actions {
      let { start, end, action: action2 } = sub_action
      let last_pos_code = if start == 0 {
        "_last_pos"
      } else {
        "_args[\{start - 1}].2"
      }
      output.write_string(
        (
          $|  let _sub_action_\{index}_result = yy_action_\{get_action_id(action2)}(\{last_pos_code}, _args[\{start}:\{end}])
          $|
        ),
      )
    }
    for binding in action.bindings {
      match binding.0 {
        Data(index, type_~) => {
          let data_type = type_
          if data_type is Constr(pkg=None, "Unit", []) {
            output.write_string(
              (
                $|  let \{binding.1} = ()
                $|
              ),
            )
          } else {
            output.write_string(
              (
                $|  guard _args[\{index}].0 is YYObj_\{derive_type_ident(data_type)}(\{binding.1})
                $|
              ),
            )
          }
        }
        StartPos =>
          // action.arity possibly wrong here because of inline actions
          output.write_string(
            (
              $|  let \{binding.1} = if _args.length() == 0 { _last_pos } else { _args[0].1 }
              $|
            ),
          )
        EndPos =>
          // action.arity possibly wrong here because of inline actions
          output.write_string(
            (
              $|  let \{binding.1} = if _args.length() == 0 { _last_pos } else { _args[_args.length() - 1].2 }
              $|
            ),
          )
        StartPosOf(index) | EndPosOf(index) =>
          if action.arity == 0 {
            output.write_string(
              (
                $|  let \{binding.1} = _last_pos
                $|
              ),
            )
          } else {
            let field = match binding.0 {
              StartPosOf(_) => 1
              EndPosOf(_) => 2
              _ => panic()
            }
            output.write_string(
              (
                $|  let \{binding.1} = _args[\{index}].\{field}
                $|
              ),
            )
          }
        SymbolStartPos => {
          used_runtime_funcs.add("_get_symbol_start_pos")
          output.write_string(
            (
              $|  let \{binding.1} = _get_symbol_start_pos(_args, _last_pos)
              $|
            ),
          )
        }
        SubAction(index~, type_~) => {
          let data_type = type_
          if data_type is Constr(pkg=None, "Unit", []) {
            output.write_string(
              (
                $|  let \{binding.1} = ()
                $|
              ),
            )
          } else {
            output.write_string(
              (
                $|  guard _sub_action_\{index}_result is YYObj_\{derive_type_ident(data_type)}(\{binding.1})
                $|
              ),
            )
          }
        }
      }
    }
    let result_data_type = action.type_
    output.write_string("  YYObj_\{derive_type_ident(result_data_type)}({(); ")
    for part in action.body {
      let (code, original_range) = part
      match original_range {
        None => ()
        Some((original_utf8_pos, original_utf8_len)) => {
          let generated_utf8_pos = output.cursor()
          match source_map_builder {
            None => ()
            Some(source_map_builder) =>
              if code.length() == original_utf8_len {
                source_map_builder.add_mapping(
                  grammar_filename,
                  original_utf8_pos,
                  generated_utf8_pos,
                  code.length(),
                )
              } else {
                // TODO: make source-map support this case
              }
          }
        }
      }
      output.write_string(code)
    }
    output.write_string("})\n")
    output.write_string(
      (
        $|}
        $|
        $|
      ),
    )
  }

  // yy_input
  output.write_string(
    (
      $|fn yy_input(token : Token, _start_pos : \{meta.position_data_type}, _end_pos : \{meta.position_data_type}) -> (YYSymbol, YYObj) {
      $|  match token {
      $|
    ),
  )
  for term in grammar.terminals {
    let meta = terminal_meta(term.name)
    match mode {
      Default =>
        if meta.data_type is Constr(pkg=None, "Unit", []) {
          output.write_string(
            (
              $|    \{term.name} => (\{CodegenSymbol::terminal(term).encode(grammar)}, YYObj_Void)
              $|
            ),
          )
        } else {
          output.write_string(
            (
              $|    \{term.name}(data) => (\{CodegenSymbol::terminal(term).encode(grammar)}, YYObj_\{derive_type_ident(meta.data_type)}(data))
              $|
            ),
          )
        }
      JsonCst(_) => {
        let payload_code = if meta.data_type is Constr(pkg=None, "Unit", []) {
          ""
        } else {
          "(data)"
        }
        let data_code = if meta.data_type is Constr(pkg=None, "Unit", []) {
          "Json::null()"
        } else {
          "data.to_json()"
        }
        output.write_string(
          (
            $|    \{term.name}\{payload_code} => (\{CodegenSymbol::terminal(term).encode(grammar)}, YYObj_Json({
            $|      "type": "TERMINAL",
            $|      "name": "\{term.name}",
            $|      "data": \{data_code},
            $|      "start": _start_pos.to_json(),
            $|      "end": _end_pos.to_json()
            $|    }))
            $|
          ),
        )
      }
    }
  }
  output.write_string(
    (
      $|  }
      $|}
      $|
      $|
    ),
  )
  let used_productions = @sorted_set.new()
  for state in automaton.states {
    for _, decision in state.action {
      if decision is Reduce(production) {
        used_productions.add(production)
      }
    }
  }
  let used_productions = used_productions.to_array()
  fn get_used_production_num(prod : @grm.Production) -> Int {
    used_productions.search(prod).unwrap()
  }

  fn encode_decision(decision : CodegenDecision) -> Int {
    match decision {
      Error => 0
      Accept => 1
      Shift(next_state) => 2 + next_state.num
      Reduce(production) =>
        2 + automaton.states.length() + get_used_production_num(production)
      ReduceNoLookahead(production) =>
        2 +
        automaton.states.length() +
        used_productions.length() +
        get_used_production_num(production)
    }
  }

  // yy_prod_lhs_table
  output.write_string(
    (
      $|let yy_prod_lhs_table : FixedArray[Int] = [
    ),
  )
  for index, production in used_productions {
    if index > 0 {
      output.write_string(", ")
    }
    output.write_string(
      "\{CodegenSymbol::nonterminal(production.lhs).encode(grammar)}",
    )
  }
  output.write_string(
    (
      $|]
      $|
      $|
    ),
  )

  // yy_prod_len_table
  output.write_string(
    (
      $|let yy_prod_len_table : FixedArray[Int] = [
    ),
  )
  for index, production in used_productions {
    if index > 0 {
      output.write_string(", ")
    }
    output.write_string("\{production.rhs.length()}")
  }
  output.write_string(
    (
      $|]
      $|
      $|
    ),
  )

  // yy_prod_action_table
  output.write_string(
    (
      $|let yy_prod_act_table : FixedArray[YYAction] = [
    ),
  )
  fn get_action_id_by_prod_num(prod_num : Int) {
    get_action_id(production_meta(prod_num).action)
  }

  for index, production in used_productions {
    if index > 0 {
      output.write_string(", ")
    }
    output.write_string(
      "yy_action_\{get_action_id_by_prod_num(production.num)}",
    )
  }
  output.write_string(
    (
      $|]
      $|
      $|
    ),
  )

  // yy_state_xxx
  let state_table = []
  fn iter_symbol() -> Iter[CodegenSymbol] {
    Iter::singleton(CodegenSymbol::eoi())
    .add(grammar.terminals.iter().map(t => CodegenSymbol::terminal(t)))
    .add(grammar.nonterminals.iter().map(nt => CodegenSymbol::nonterminal(nt)))
  }

  fn emit_state(state : @lr1.LR1State) {
    let decision_groups : @sorted_map.SortedMap[
      CodegenDecision,
      @immut/sorted_set.SortedSet[CodegenSymbol],
    ] = @sorted_map.new()
    fn add_symbol_decision(symbol, decision) {
      decision_groups[decision] = decision_groups
        .get(decision)
        .unwrap_or(@immut/sorted_set.new())
        .add(symbol)
    }

    for input, decision in state.action {
      add_symbol_decision(
        match input {
          Input(term) => T(term)
          EndOfInput => EOI
        },
        match decision {
          Accept => Accept
          Shift(next_state) => Shift(next_state)
          Reduce(production) => Reduce(production)
          Conflict(_) => panic()
        },
      )
    }
    for symbol, state in state.goto {
      match symbol {
        NT(nonterm) => add_symbol_decision(NT(nonterm), Shift(state))
        _ => ()
      }
    }
    let no_lookahead_needed = match [..decision_groups.keys_as_iter()][0] {
      Reduce(_) | Accept => true
      _ => false
    }
    fn to_codegen_decision(decision : @lr1.Decision?) -> CodegenDecision {
      match decision {
        None => Error
        Some(Accept) => Accept
        Some(Shift(next_state)) => Shift(next_state)
        Some(Reduce(production)) =>
          if no_lookahead_needed {
            ReduceNoLookahead(production)
          } else {
            Reduce(production)
          }
        Some(Conflict(_)) => panic()
      }
    }

    for symbol in iter_symbol() {
      let decision : CodegenDecision = match symbol {
        EOI => to_codegen_decision(state.action.get(EndOfInput))
        T(t) => to_codegen_decision(state.action.get(Input(t)))
        NT(nt) =>
          match state.goto.get(NT(nt)) {
            None => Error
            Some(next_state) => Shift(next_state)
          }
      }
      state_table.push(encode_decision(decision))
    }
  }

  for state in automaton.states {
    emit_state(state)
  }

  // Assume input elements are unsigned
  fn compress_array(input : Array[Int]) -> Array[Int] {
    if input.length() == 0 {
      return []
    }
    let output = []
    let mut last = input[0]
    let mut count = 1
    for val in input[1:] {
      if val == last {
        count += 1
      } else {
        output.push(last)
        if count > 1 {
          output.push(-count)
        }
        last = val
        count = 1
      }
    }
    output
  }

  let state_table = if compress {
    compress_array(state_table)
  } else {
    state_table
  }
  if compress {
    output.write_string(
      (
        $|fn yy_decompress(input : FixedArray[Int]) -> FixedArray[Int] {
        $|  let output = []
        $|  let mut last = 0
        $|  for val in input {
        $|    if val < 0 {
        $|      let count = -val - 1
        $|      for _ in 0..<count {
        $|        output.push(last)
        $|      }
        $|    } else {
        $|      output.push(val)
        $|      last = val
        $|    }
        $|  }
        $|  FixedArray::from_array(output)
        $|}
        $|
        $|let yy_state_table : FixedArray[Int] = yy_decompress([
      ),
    )
  } else {
    output.write_string(
      (
        $|let yy_state_table : FixedArray[Int] = [
      ),
    )
  }
  for index, state in state_table {
    if index > 0 {
      output.write_string(", ")
    }
    output.write_string("\{state}")
  }
  if compress {
    output.write_string(
      (
        $|])
        $|
        $|
      ),
    )
  } else {
    output.write_string(
      (
        $|]
        $|
        $|
      ),
    )
  }
  let symbol_cardinality = 1 +
    grammar.terminals.length() +
    grammar.nonterminals.length()
  output.write_string(
    (
      $|fn yy_state(state : YYState, symbol : YYSymbol) -> YYDecision {
      $|  let encoded = yy_state_table[state * \{symbol_cardinality} + symbol]
      $|  match encoded {
      $|    0 => Error
      $|    1 => Accept
      $|    2..=\{2 + automaton.states.length() - 1} => Shift(encoded - 2)
      $|    \{2 + automaton.states.length()}..=\{2 + automaton.states.length() + used_productions.length() - 1} =>
      $|      Reduce(
      $|        yy_prod_len_table[encoded - \{2 + automaton.states.length()}],
      $|        yy_prod_lhs_table[encoded - \{2 + automaton.states.length()}],
      $|        yy_prod_act_table[encoded - \{2 + automaton.states.length()}],
      $|      )
      $|    \{2 + automaton.states.length() + used_productions.length()}..<_ =>
      $|      ReduceNoLookahead(
      $|        yy_prod_len_table[encoded - \{2 + automaton.states.length() + used_productions.length()}],
      $|        yy_prod_lhs_table[encoded - \{2 + automaton.states.length() + used_productions.length()}],
      $|        yy_prod_act_table[encoded - \{2 + automaton.states.length() + used_productions.length()}],
      $|      )
      $|    _ => panic()
      $|  }
      $|}
      $|
      $|
    ),
  )

  // yy_parse
  match input_mode {
    Array =>
      output.write_string(
        (
          $|fn[T] yy_parse(
          $|  tokens : Array[(Token, \{meta.position_data_type}, \{meta.position_data_type})],
          $|  start : YYState,
          $|  return_ : (YYObj) -> T,
          $|  initial_pos? : \{meta.position_data_type},
          $|) -> T raise ParseError {
          $|  let mut cursor = 0
          $|  let mut state_stack : @list.List[YYState] = @list.cons(start, @list.empty())
          $|  let data_stack : Array[(YYObj, \{meta.position_data_type}, \{meta.position_data_type})] = []
          $|  let mut last_pos = initial_pos.unwrap_or(tokens[0].1)
          $|  let mut state = start
          $|  let mut lookahead : Option[(YYSymbol, (YYObj, \{meta.position_data_type}, \{meta.position_data_type}), Token?)] = None
          $|  let mut last_shifted_state_stack = state_stack
          $|  while true {
          $|    let decision = match yy_state(state, \{CodegenSymbol::eoi().encode(grammar)}) {
          $|      ReduceNoLookahead(_) | Accept as t => t
          $|      _ => {
          $|        match lookahead {
          $|          Some(la) => yy_state(state, la.0)
          $|          None => {
          $|            if cursor < tokens.length() {
          $|              let (token, start_pos, end_pos) = tokens[cursor]
          $|              cursor += 1
          $|              let (symbol, data) = yy_input(token, start_pos, end_pos)
          $|              lookahead = Some((symbol, (data, start_pos, end_pos), Some(token)))
          $|              yy_state(state, symbol)
          $|            } else {
          $|              lookahead = Some((\{CodegenSymbol::eoi().encode(grammar)}, (YYObj_Void, last_pos, last_pos), None))
          $|              yy_state(state, \{CodegenSymbol::eoi().encode(grammar)})
          $|            }
          $|          }
          $|        }
          $|      }
          $|    }
          $|    match decision {
          $|      Accept => return return_(data_stack.unsafe_pop().0)
          $|      Shift(next_state) => {
          $|        guard lookahead is Some(la)
          $|        data_stack.push(la.1)
          $|        state_stack = @list.cons(next_state, state_stack)
          $|        last_shifted_state_stack = state_stack
          $|        state = next_state
          $|        last_pos = la.1.2
          $|        lookahead = None
          $|      }
          $|      Reduce(count, symbol, action)
          $|      | ReduceNoLookahead(count, symbol, action) => {
          $|        loop (count, symbol, action) {
          $|          _ => {
          $|            let args = data_stack[data_stack.length() - count:]
          $|            let data = action(last_pos, args)
          $|            let (start_pos, end_pos) = if args.length() == 0 {
          $|              (last_pos, last_pos)
          $|            } else {
          $|              (args[0].1, args[args.length() - 1].2)
          $|            }
          $|            for i in 0..<count {
          $|              ignore(data_stack.unsafe_pop())
          $|              state_stack = state_stack.unsafe_tail()
          $|            }
          $|            state = state_stack.unsafe_head()
          $|            data_stack.push((data, start_pos, end_pos))
          $|            match yy_state(state, symbol) {
          $|              Accept => return return_(data_stack.unsafe_pop().0)
          $|              Shift(next_state) => {
          $|                state_stack = @list.cons(next_state, state_stack)
          $|                state = next_state
          $|              }
          $|              Reduce(count, symbol, action)
          $|              | ReduceNoLookahead(count, symbol, action) => continue (count, symbol, action)
          $|              _ => panic()
          $|            }
          $|          }
          $|        }
          $|      }
          $|      Error => {
          $|        let (_, (_, start_pos, end_pos), token) = lookahead.unwrap()
          $|        error(last_shifted_state_stack, token, (start_pos, end_pos))
          $|      }
          $|    }
          $|  }
          $|  panic()
          $|}
          $|
          $|
        ),
      )
    Pull =>
      output.write_string(
        (
          $|fn[T] yy_parse(
          $|  read_token : () -> (Token, \{meta.position_data_type}, \{meta.position_data_type}),
          $|  start_pos : \{meta.position_data_type},
          $|  start : YYState,
          $|  return_ : (YYObj) -> T
          $|) -> T raise ParseError {
          $|  let mut state_stack : @list.List[YYState] = @list.cons(start, @list.empty())
          $|  let data_stack : Array[(YYObj, \{meta.position_data_type}, \{meta.position_data_type})] = []
          $|  let mut last_pos = start_pos
          $|  let mut state = start
          $|  let mut lookahead : Option[(YYSymbol, (YYObj, \{meta.position_data_type}, \{meta.position_data_type}), Token)] = None
          $|  let mut last_shifted_state_stack = state_stack
          $|  while true {
          $|    let decision = match yy_state(state, \{CodegenSymbol::eoi().encode(grammar)}) {
          $|      ReduceNoLookahead(_) | Accept as t => t
          $|      _ => {
          $|        match lookahead {
          $|          Some(la) => yy_state(state, la.0)
          $|          None => {
          $|            let (token, start_pos, end_pos) = read_token()
          $|            let (symbol, data) = yy_input(token, start_pos, end_pos)
          $|            lookahead = Some((symbol, (data, start_pos, end_pos), token))
          $|            yy_state(state, symbol)
          $|          }
          $|        }
          $|      }
          $|    }
          $|    match decision {
          $|      Accept => return return_(data_stack.unsafe_pop().0)
          $|      Shift(next_state) => {
          $|        guard lookahead is Some(la)
          $|        data_stack.push(la.1)
          $|        state_stack = @list.cons(next_state, state_stack)
          $|        last_shifted_state_stack = state_stack
          $|        state = next_state
          $|        last_pos = la.1.2
          $|        lookahead = None
          $|      }
          $|      Reduce(count, symbol, action)
          $|      | ReduceNoLookahead(count, symbol, action) => {
          $|        loop (count, symbol, action) {
          $|          _ => {
          $|            let args = data_stack[data_stack.length() - count:]
          $|            let data = action(last_pos, args)
          $|            let (start_pos, end_pos) = if args.length() == 0 {
          $|              (last_pos, last_pos)
          $|            } else {
          $|              (args[0].1, args[args.length() - 1].2)
          $|            }
          $|            for i in 0..<count {
          $|              ignore(data_stack.unsafe_pop())
          $|              state_stack = state_stack.unsafe_tail()
          $|            }
          $|            state = state_stack.unsafe_head()
          $|            data_stack.push((data, start_pos, end_pos))
          $|            match yy_state(state, symbol) {
          $|              Accept => return return_(data_stack.unsafe_pop().0)
          $|              Shift(next_state) => {
          $|                state_stack = @list.cons(next_state, state_stack)
          $|                state = next_state
          $|              }
          $|              Reduce(count, symbol, action)
          $|              | ReduceNoLookahead(count, symbol, action) => continue (count, symbol, action)
          $|              _ => panic()
          $|            }
          $|          }
          $|        }
          $|      }
          $|      Error => {
          $|        let (_, (_, start_pos, end_pos), token) = lookahead.unwrap()
          $|        error(last_shifted_state_stack, token, (start_pos, end_pos))
          $|      }
          $|    }
          $|  }
          $|  panic()
          $|}
          $|
          $|
        ),
      )
  }
  match input_mode {
    Array =>
      output.write_string(
        (
          $|fn error(stack : @list.List[YYState], token : Token?, loc : (\{meta.position_data_type}, \{meta.position_data_type})) -> Unit raise ParseError {
          $|
        ),
      )
    Pull =>
      output.write_string(
        (
          $|fn error(stack : @list.List[YYState], token : Token, loc : (\{meta.position_data_type}, \{meta.position_data_type})) -> Unit raise ParseError {
          $|
        ),
      )
  }
  output.write_string(
    (
      $|  let expected = []
      $|  fn try_add(symbol : YYSymbol, kind : TokenKind) {
      $|    fn go(stack : @list.List[YYState]) {
      $|      match stack {
      $|        Empty => ()
      $|        More(state, ..) => {
      $|          match yy_state(state, symbol) {
      $|            Accept | Shift(_) => expected.push(kind)
      $|            Reduce(count, symbol, _) | ReduceNoLookahead(count, symbol, _) => {
      $|              fn inner_go(stack : @list.List[YYState], count, symbol) {
      $|                let stack = stack.drop(count)
      $|                guard stack is More(state, ..)
      $|                match yy_state(state, symbol) {
      $|                  Shift(state) => go(@list.cons(state, stack))
      $|                  Reduce(count, symbol, _) | ReduceNoLookahead(count, symbol, _) => inner_go(stack, count, symbol)
      $|                  _ => panic()
      $|                }
      $|              }
      $|              inner_go(stack, count, symbol)
      $|            }
      $|            Error => ()
      $|          }
      $|        }
      $|      }
      $|    }
      $|    go(stack)
      $|  }
      $|
    ),
  )
  // Note: this is a workaround since moonc will stack overflow if the code is too long like this
  // ```
  // for term in grammar.terminals {
  //   output.write_string(
  //     $|  try_add(T_\{term.name}, TK_\{term.name})
  //     $|
  //     ,
  //   )
  // }
  // ```
  output.write_string("  for term in ([")
  for i, term in grammar.terminals {
    if i > 0 {
      output.write_string(", ")
    }
    output.write_string(
      "(\{CodegenSymbol::terminal(term).encode(grammar)}, TK_\{term.name})",
    )
  }
  output.write_string("] : Array[(YYSymbol, TokenKind)]) {\n")
  output.write_string(
    (
      $|    try_add(term.0, term.1)
      $|  }
      $|
    ),
  )
  match input_mode {
    Array =>
      output.write_string(
        (
          $|  match token {
          $|    None => raise UnexpectedEndOfInput(loc.1, expected)
          $|    Some(token) => raise UnexpectedToken(token, loc, expected)
          $|  }
          $|
        ),
      )
    Pull =>
      output.write_string(
        (
          $|  raise UnexpectedToken(token, loc, expected)
          $|
        ),
      )
  }
  output.write_string(
    (
      $|}
      $|
      $|
    ),
  )
  for start in automaton.starts {
    let (start_production, start_state) = start
    let name = start_production.lhs.name
    let original_name = try! name[:name.length() - "_prime".length()].to_string()
    match input_mode {
      Array =>
        output.write_string(
          (
            $|pub fn \{original_name}(tokens : Array[(Token, \{meta.position_data_type}, \{meta.position_data_type})], initial_pos? : \{meta.position_data_type}) -> \{fix_result_type(nonterminal_meta(name).data_type.to_string())} raise ParseError {
            $|  yy_parse(
            $|    tokens,
            $|    \{start_state.num},
            $|    (it) => {
            $|      guard it is YYObj_\{derive_type_ident(nonterminal_meta(name).data_type)}(result)
            $|      result
            $|    },
            $|    initial_pos?,
            $|  )
            $|}
            $|
          ),
        )
      Pull =>
        output.write_string(
          (
            $|pub fn \{original_name}(read_token : () -> (Token, \{meta.position_data_type}, \{meta.position_data_type}), start_pos : \{meta.position_data_type}) -> \{fix_result_type(nonterminal_meta(name).data_type.to_string())} raise ParseError {
            $|  yy_parse(
            $|    read_token,
            $|    start_pos,
            $|    \{start_state.num},
            $|    (it) => {
            $|      guard it is YYObj_\{derive_type_ident(nonterminal_meta(name).data_type)}(result)
            $|      result
            $|    },
            $|  )
            $|}
            $|
          ),
        )
    }
  }

  // Runtime functions
  for func in used_runtime_funcs {
    match func {
      "_get_symbol_start_pos" =>
        output.write_string(
          (
            $|
            $|fn _get_symbol_start_pos(args : ArrayView[(YYObj, \{meta.position_data_type}, \{meta.position_data_type})], last_pos : \{meta.position_data_type}) -> \{meta.position_data_type} {
            $|  if args.length() == 0 {
            $|    last_pos
            $|  } else {
            $|    for i = 0; i < args.length(); i = i + 1 {
            $|      let (_, start_pos, end_pos) = args[i]
            $|      if start_pos == end_pos {
            $|        continue
            $|      }
            $|      return start_pos
            $|    }
            $|    args[args.length() - 1].2
            $|  }
            $|}
            $|
          ),
        )
      _ => panic()
    }
  }
  match mode {
    Default => ()
    JsonCst(_) =>
      output.write_string(
        (
          $|
          $|fn args_to_json(args : ArrayView[(YYObj, \{meta.position_data_type}, \{meta.position_data_type})]) -> Json {
          $|  Json::array(args.iter().map((it) => {
          $|    guard it.0 is YYObj_Json(json)
          $|    json
          $|  }).to_array())
          $|}
          $|
        ),
      )
  }

  // Footer
  for chunk in meta.footer {
    let (code, original_range) = chunk
    match (source_map_builder, original_range) {
      (None, _) | (_, None) => ()
      (Some(source_map_builder), Some((original_utf8_pos, utf8_len))) => {
        let generated_utf8_pos = output.cursor()
        source_map_builder.add_mapping(
          grammar_filename, original_utf8_pos, generated_utf8_pos, utf8_len,
        )
      }
    }
    output.write_string(code)
  }
}

///|
priv struct GeneratorConfig {
  compress : Bool
}

///|
priv struct CodeGenerator(GeneratorConfig)

///|
impl @codegen.CodeGenerator for CodeGenerator with void_type(self) {
  ignore(self)
  "Unit"
}

///|
impl @codegen.CodeGenerator for CodeGenerator with void_action_code(self) {
  ignore(self)
  "()"
}

///|
impl @codegen.CodeGenerator for CodeGenerator with codegen_tokens(
  self,
  terminals,
  terminal_meta,
  output,
  no_comments~,
  derive_map~,
) {
  ignore(self)
  codegen_tokens(terminals, terminal_meta, output, no_comments~, derive_map~)
}

///|
impl @codegen.CodeGenerator for CodeGenerator with codegen(
  self,
  grammar,
  automaton,
  meta,
  output,
  source_map_builder~,
  grammar_filename~,
  external_tokens~,
  no_comments~,
  mode~,
  input_mode~,
) {
  ignore(self)
  codegen(
    grammar,
    automaton,
    meta,
    output,
    source_map_builder?,
    grammar_filename~,
    external_tokens~,
    no_comments~,
    mode~,
    input_mode~,
    compress=self.compress,
  )
}

///|
pub let compress_generator : &@codegen.CodeGenerator = CodeGenerator({
  compress: true,
})

///|
pub let generator : &@codegen.CodeGenerator = CodeGenerator({ compress: false })

///|
fn fix_result_type(str : String) -> String {
  if str.contains("->") {
    "(\{str})"
  } else {
    str
  }
}
