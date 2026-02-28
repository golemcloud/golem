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
using @util {println_to_stderr, exit, path_basename}

///|
pub(all) enum Mode {
  Default
  JsonCst
  OnlyTokens
}

///|
pub(all) enum TokenPayloadRewrite {
  NoPayload
  JsonPayload
  NoRewrite
}

///|
pub(all) enum InputMode {
  Array
  Pull
}

///|
pub fn print(
  parser_spec_str : String,
  filename~ : String,
  out~ : StringBuilder,
) -> Unit {
  let lexer = @parser.new_lexer(parser_spec_str)
  fn token() {
    lexer.next_token() catch {
      @parser.Unrecognized(x, pos) => {
        let pos_str = pos_to_string(filename, parser_spec_str, pos)
        println_to_stderr(
          "SyntaxError: Unrecognized character \{([x] : String).escape()}, at \{pos_str}",
        )
        exit(1)
      }
      @parser.UnexpectedEndOfFile => {
        println_to_stderr("SyntaxError: Unexpected end of file")
        exit(1)
      }
    }
  }

  let spec = @parser.spec(token, 0) catch {
    @parser.UnexpectedToken(token, loc, expected) => {
      let loc_str = loc_to_string(filename, parser_spec_str, loc)
      let expected_str = array_to_or_list(expected.map(Show::to_string)[:])
      println_to_stderr(
        "SyntaxError: Unexpected token \{token.kind()}, expected \{expected_str}.\n  at \{loc_str}",
      )
      exit(1)
    }
  }
  @ast.print_spec(spec, out)
  println(out.to_string())
}

///|
pub fn compile(
  parser_spec_str : String,
  mode? : Mode = Default,
  input_mode? : InputMode = Array,
  filename~ : String,
  external_tokens? : Bool = false,
  no_comments? : Bool = false,
  no_std? : Bool = false,
  token_payload_rewrite? : TokenPayloadRewrite = NoRewrite,
  force_int_position? : Bool = false,
  source_map_builder? : &@codegen.SourceMapBuilder,
  generator~ : &@codegen.CodeGenerator,
) -> String {
  let lexer = @parser.new_lexer(parser_spec_str)
  fn token() {
    lexer.next_token() catch {
      @parser.Unrecognized(x, pos) => {
        let pos_str = pos_to_string(filename, parser_spec_str, pos)
        println_to_stderr(
          "SyntaxError: Unrecognized character \{([x] : String).escape()}, at \{pos_str}",
        )
        exit(1)
      }
      @parser.UnexpectedEndOfFile => {
        println_to_stderr("SyntaxError: Unexpected end of file")
        exit(1)
      }
    }
  }

  let spec = @parser.spec(token, 0) catch {
    @parser.UnexpectedToken(token, loc, expected) => {
      let loc_str = loc_to_string(filename, parser_spec_str, loc)
      let expected_str = array_to_or_list(expected.map(Show::to_string)[:])
      println_to_stderr(
        "SyntaxError: Unexpected token \{token.kind()}, expected \{expected_str}.\n  at \{loc_str}",
      )
      exit(1)
    }
  }
  let spec = @elab.elaborate_with_stdlib_rules(
    spec,
    parser_spec_str,
    path_basename(filename),
    json_cst=match mode {
      JsonCst => Yes
      Default | OnlyTokens => No
    },
    no_std~,
    token_payload_rewrite=match token_payload_rewrite {
      NoRewrite => NoRewrite
      JsonPayload => JsonPayload
      NoPayload => NoPayload
    },
    force_int_position~,
  ) catch {
    @elab.UnresolvedSymbol(symbol, loc~) => {
      let loc_str = loc_to_string(filename, parser_spec_str, loc)
      println_to_stderr("Unresolved symbol \{symbol}, at \{loc_str}")
      exit(1)
    }
    // TODO: Add location information
    @elab.FailedToParseTypeExpr(type_expr) => {
      let type_expr_str = type_expr.to_string()
      println_to_stderr("Failed to parse type expression \{type_expr_str}")
      exit(1)
    }
    @elab.OutOfBoundsAccess(index, loc~) => {
      let loc_str = loc_to_string(filename, parser_spec_str, loc)
      println_to_stderr("Out of bounds access \{index}, at \{loc_str}")
      exit(1)
    }
  }
  let spec = @desugar.eliminate_rule_args(
    spec,
    json_cst=match mode {
      JsonCst => Yes
      Default | OnlyTokens => No
    },
  )
  let spec = @desugar.eliminate_inline(spec)
  let terminals = []
  let nonterminals = []
  let terminal_by_name : Map[String, @grm.Terminal] = {}
  let nonterminal_by_name : Map[String, @grm.Nonterminal] = {}
  for token in spec.tokens {
    let terminal : @grm.Terminal = {
      name: token.name,
      num: terminals.length(),
      prec: match token.prec {
        None => None
        Some((prec, NonAssoc)) => Some((prec, @grm.NonAssoc))
        Some((prec, LeftAssoc)) => Some((prec, @grm.LeftAssoc))
        Some((prec, RightAssoc)) => Some((prec, @grm.RightAssoc))
      },
    }
    terminal_by_name[token.name] = terminal
    terminals.push(terminal)
  }
  for rule_name, rule in spec.rules {
    let nonterminal : @grm.Nonterminal = {
      name: rule_name,
      num: nonterminals.length(),
      productions: [],
    }
    nonterminal_by_name[rule.name] = nonterminal
    nonterminals.push(nonterminal)
  }
  fn get_terminal_by_name(name : String) -> @grm.Terminal {
    terminal_by_name.get(name).unwrap()
  }

  fn get_nonterminal_by_name(name : String) -> @grm.Nonterminal {
    nonterminal_by_name.get(name).unwrap()
  }

  let production_meta_map : Map[Int, @codegen.ProductionMeta] = {}
  let terminal_meta_map : Map[Int, @codegen.TerminalMeta] = {}
  let nonterminal_meta_map : Map[Int, @codegen.NonTerminalMeta] = {}
  let productions = []
  let starts : Array[String] = spec.start_rules
  let position_data_type = spec.position_type
  let derive_map = spec.derive_map
  for token in spec.tokens {
    let terminal = get_terminal_by_name(token.name)
    terminal_meta_map[terminal.num] = {
      data_type: token.type_,
      image: token.image,
    }
  }
  match mode {
    OnlyTokens => {
      let output = StringBuilder::new()
      generator.codegen_tokens(
        terminals,
        fn(name) {
          terminal_meta_map.get_or_init(get_terminal_by_name(name).num, fn() {
            { data_type: Constr(pkg=None, "Unit", []), image: None }
          })
        },
        output,
        no_comments~,
        derive_map=spec.derive_map,
      )
      return output.to_string()
    }
    _ => ()
  }
  for rule_name, rule in spec.rules {
    let lhs = get_nonterminal_by_name(rule_name)
    nonterminal_meta_map[lhs.num] = { data_type: rule.type_ }
    for clause in rule.clauses {
      let production_num = productions.length()
      let production : @grm.Production = {
        num: production_num,
        lhs,
        rhs: clause.items.map(fn(item) {
          match item.term {
            Token(token) => T(get_terminal_by_name(token.name))
            RuleCall(rule_name, [], _type) =>
              NT(get_nonterminal_by_name(rule_name))
            Param(_) | RuleCall(_, _, _) => panic()
          }
        }),
        prec: clause.prec,
      }
      productions.push(production)
      lhs.productions.push(production)
      production_meta_map[production_num] = { action: clause.action }
    }
  }
  let starts = starts.map(fn(name) {
    let start_nt = get_nonterminal_by_name(name)
    let augmented_start_nt : @grm.Nonterminal = {
      num: nonterminals.length(),
      name: "\{name}_prime",
      productions: [],
    }
    nonterminal_by_name[augmented_start_nt.name] = augmented_start_nt
    nonterminals.push(augmented_start_nt)
    nonterminal_meta_map[augmented_start_nt.num] = nonterminal_meta_map
      .get(start_nt.num)
      .unwrap()
    let production : @grm.Production = {
      num: productions.length(),
      lhs: augmented_start_nt,
      rhs: [NT(start_nt)],
      prec: None,
    }
    productions.push(production)
    augmented_start_nt.productions.push(production)
    production
  })

  // fix prec for productions
  for prod in productions {
    match prod.prec {
      Some(_) => ()
      None => {
        let rightmost_term = prod.rhs.rev_iter().find_first(fn(x) { x is T(_) })
        match rightmost_term {
          Some(T(terminal)) => {
            let term = get_terminal_by_name(terminal.name)
            prod.prec = match term.prec {
              Some((prec, _)) => Some(prec)
              None => None
            }
          }
          Some(NT(_)) => panic()
          None => ()
        }
      }
    }
  }
  // for prod in productions {
  //   println(prod)
  // }
  let grammar : @grm.Grammar = { terminals, nonterminals, productions, starts }
  let automaton = @lr1.Automaton::build(grammar, user_eoi=input_mode is Pull)
  let errors = @lr1.resolve_conflicts(automaton.conflicts)
  for error in errors {
    // TODO: detailed error message
    match error {
      Reduce_conflict_resolved_by_presentation_order(_) =>
        println_to_stderr("Reduce conflict resolved by presentation order")
      Shift_reduce_conflict_resolved_without_precedence(_) =>
        println_to_stderr("Shift-reduce conflict resolved without precedence")
      Shift_reduce_conflict_not_resolved_because_of_non_assoc(_) =>
        println_to_stderr(
          "Shift-reduce conflict not resolved because of non-associativity",
        )
    }
  }
  let meta : @codegen.MetaProvider = {
    header: spec.header,
    footer: spec.trailer,
    position_data_type,
    terminal_meta: fn(name) {
      terminal_meta_map.get_or_init(get_terminal_by_name(name).num, fn() {
        { data_type: Constr(pkg=None, "Unit", []), image: None }
      })
    },
    nonterminal_meta: fn(name) {
      nonterminal_meta_map.get(get_nonterminal_by_name(name).num).unwrap()
    },
    production_meta: fn(num) { production_meta_map.get(num).unwrap() },
    derive_map,
  }
  let output_buffer = StringBuilder::new()
  let output = @logger_with_cursor.new(output_buffer)
  generator.codegen(
    grammar,
    automaton,
    meta,
    output,
    source_map_builder~,
    grammar_filename=path_basename(filename),
    mode=match mode {
      OnlyTokens => panic()
      Default => Default
      JsonCst => JsonCst
    },
    input_mode=match input_mode {
      Array => Array
      Pull => Pull
    },
    external_tokens~,
    no_comments~,
  )
  output_buffer.to_string()
}
