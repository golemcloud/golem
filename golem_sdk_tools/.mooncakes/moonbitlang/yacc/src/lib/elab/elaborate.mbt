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
pub suberror ElabError {
  UnresolvedSymbol(String, loc~ : (Int, Int))
  FailedToParseTypeExpr(String)
  OutOfBoundsAccess(Int, loc~ : (Int, Int))
}

///|
pub(all) enum JsonCst {
  No
  Yes
}

///|
pub(all) enum TokenPayloadRewrite {
  NoPayload
  JsonPayload
  NoRewrite
}

///|
fn build_rules_from_src(
  src : String,
  filename~ : String,
  json_cst~ : JsonCst,
  token_payload_rewrite~ : TokenPayloadRewrite,
  force_int_position~ : Bool,
) -> Map[String, Rule] {
  let lexer = @parser.new_lexer(src)
  fn token() {
    try! lexer.next_token()
  }

  let ast_spec = try! @parser.spec(token, 0)
  let spec = try! elaborate(
    ast_spec,
    src,
    filename,
    json_cst~,
    token_payload_rewrite~,
    force_int_position~,
  )
  spec.rules
}

///|
pub fn elaborate_with_stdlib_rules(
  ast_spec : @ast.ParserSpec,
  parser_spec_str : String,
  filename : String,
  json_cst~ : JsonCst,
  no_std~ : Bool,
  token_payload_rewrite~ : TokenPayloadRewrite,
  force_int_position~ : Bool,
) -> ParserSpec raise ElabError {
  elaborate(
    ast_spec,
    parser_spec_str,
    filename,
    json_cst~,
    stdlib_rules=if no_std {
      {}
    } else {
      build_rules_from_src(
        stdlib_src,
        filename="stdlib.mbty",
        json_cst~,
        token_payload_rewrite~,
        force_int_position~,
      )
    },
    token_payload_rewrite~,
    force_int_position~,
  )
}

///|
pub fn elaborate(
  ast_spec : @ast.ParserSpec,
  parser_spec_str : String,
  filename : String,
  json_cst~ : JsonCst,
  stdlib_rules? : Map[String, Rule],
  token_payload_rewrite~ : TokenPayloadRewrite,
  force_int_position~ : Bool,
) -> ParserSpec raise ElabError {
  let mut header = @immut/array.new()
  let mut trailer = @immut/array.new()
  for decl in ast_spec.decls {
    match decl {
      Header(code, utf8_pos, utf8_len) =>
        header = header.push((code, Some((utf8_pos, utf8_len))))
      Trailer(code, utf8_pos, utf8_len) =>
        trailer = trailer.push((code, Some((utf8_pos, utf8_len))))
      _ => ()
    }
  }
  let token_by_name : Map[String, Token] = {}
  let rule_by_name : Map[String, Rule] = {}
  let token_by_image : Map[String, Token] = {}
  let tokens : Array[Token] = []
  match stdlib_rules {
    None => ()
    Some(stdlib_rules) =>
      for rule_name, rule in stdlib_rules {
        rule_by_name[rule_name] = rule
      }
  }
  fn get_token_by_name(name : String) -> Token {
    match token_by_name.get(name) {
      Some(t) => t
      None => {
        let token : Token = {
          name,
          type_: Constr(pkg=None, "Unit", []),
          image: None,
          prec: None,
        }
        token_by_name[name] = token
        tokens.push(token)
        token
      }
    }
  }

  // Prepare tokens
  fn elaborate_token_type(
    type_ : @ast.TypeString?,
  ) -> TypeExpr raise ElabError {
    match token_payload_rewrite {
      NoRewrite =>
        match type_ {
          Some(type_) => parse_and_elaborate_type_expr(type_)
          None => Constr(pkg=None, "Unit", [])
        }
      JsonPayload => Constr(pkg=None, "Json", [])
      NoPayload => Constr(pkg=None, "Unit", [])
    }
  }

  for ast_decl in ast_spec.decls {
    match ast_decl {
      Token(names, type_~) =>
        for name in names {
          let token = get_token_by_name(name.symbol)
          token.type_ = elaborate_token_type(type_)
        }
      Token1(name, type_~, image~) => {
        let token = get_token_by_name(name.symbol)
        token.type_ = elaborate_token_type(type_)
        token.image = Some(image)
        token_by_image[image] = token
      }
      _ => ()
    }
  }

  // Prepare rules
  for ast_rule in ast_spec.rules {
    let generic_params = Set::from_array(ast_rule.generic_params)
    let params = ast_rule.params.map(fn(param) {
      let (name, type_) = param
      let type_ = match type_ {
        Some(type_) =>
          elaborate_type_expr_with_generic_params(type_, generic_params)
        None => {
          let generic_param = "_\{generic_params.length()}"
          generic_params.add(generic_param)
          Param(generic_param)
        }
      }
      (name, type_)
    })
    let type_ = if json_cst is Yes(_) {
      Constr(pkg=None, "Json", [])
    } else {
      match ast_rule.type_ {
        Some(type_) =>
          elaborate_type_expr_with_generic_params(type_, generic_params)
        None => Constr(pkg=None, "Unit", [])
      }
    }
    let rule : Rule = {
      inline: ast_rule.inline,
      name: ast_rule.nonterminal,
      generic_params,
      params,
      type_,
      clauses: [],
    }
    rule_by_name[rule.name] = rule
  }
  let mut curr_prec = 0
  let prec_map = @sorted_map.new()
  let start_rules : Array[Symbol] = []
  for ast_decl in ast_spec.decls {
    match ast_decl {
      Left(idents) | Right(idents) | Nonassoc(idents) => {
        let assoc : Associativity = match ast_decl {
          Left(_) => LeftAssoc
          Right(_) => RightAssoc
          Nonassoc(_) => NonAssoc
          _ => panic()
        }
        let prec = curr_prec
        curr_prec += 1
        for ident in idents {
          // TODO: error reporting
          match ident {
            Symbol(name) => {
              if token_by_name.get(name) is Some(token) {
                token.prec = Some((prec, assoc))
              }
              prec_map[name] = (prec, assoc)
            }
            Image(image) => {
              let token = token_by_image.get(image).unwrap()
              token.prec = Some((prec, assoc))
              prec_map[token.name] = (prec, assoc)
            }
          }
        }
      }
      Start(symbols, type_~) =>
        for symbol in symbols {
          start_rules.push(symbol.symbol)
          guard rule_by_name.get(symbol.symbol) is Some(rule) else {
            raise UnresolvedSymbol(symbol.symbol, loc=symbol.loc)
          }
          if json_cst is No && type_ is Some(type_) {
            rule.type_ = parse_and_elaborate_type_expr(type_)
          }
        }
      Type(idents, type_~) =>
        for ident in idents {
          match token_by_name.get(ident.symbol) {
            None =>
              match rule_by_name.get(ident.symbol) {
                None => panic()
                Some(rule) =>
                  // TODO: report error if rule already has a type
                  if json_cst is No {
                    rule.type_ = parse_and_elaborate_type_expr(type_)
                  }
              }
            Some(token) =>
              if token_payload_rewrite is NoRewrite {
                token.type_ = elaborate_token_type(Some(type_))
              }
          }
        }
      _ => ()
    }
  }
  fn map_term(
    ast_term : @ast.Term,
    rule_param_map : Map[String, TypeExpr],
  ) -> Term raise ElabError {
    match ast_term {
      Symbol(symbol, loc~) =>
        match token_by_name.get(symbol) {
          None =>
            match rule_param_map.get(symbol) {
              Some(type_) => Param(symbol, type_)
              None =>
                match rule_by_name.get(symbol) {
                  None => raise UnresolvedSymbol(symbol, loc~)
                  Some(rule) => RuleCall(symbol, [], rule.type_)
                }
            }
          Some(token) => Token(token)
        }
      Image(image, loc~) =>
        match token_by_image.get(image) {
          None => raise UnresolvedSymbol(image.escape(), loc~)
          Some(token) => Token(token)
        }
      RuleCall(symbol, symbol_loc~, args) =>
        match rule_by_name.get(symbol) {
          None => raise UnresolvedSymbol(symbol, loc=symbol_loc)
          Some(rule) =>
            RuleCall(
              symbol,
              map_error(args, fn(arg) { map_term(arg, rule_param_map) }),
              rule.type_,
            )
        }
    }
  }

  for rule_index, ast_rule in ast_spec.rules {
    let rule = rule_by_name.get(ast_rule.nonterminal).unwrap()
    let rule_param_map : Map[String, TypeExpr] = Map::from_array(rule.params)
    let ast_clause_iter = ast_rule.clauses
      .iter()
      .flat_map(it => {
        let (clauses, action) = it
        clauses.iter().map(fn(it) { (it, action) })
      })
    let mut clause_index = 0
    for ast_clause in ast_clause_iter {
      let (ast_clause, ast_clause_action) = ast_clause
      let items = map_error(ast_clause.items, fn(ast_item) {
        Item::{
          binder: ast_item.binder,
          term: map_term(ast_item.term, rule_param_map),
        }
      })
      let prec = match ast_clause.prec {
        Some(ident) => {
          // TODO: error reporting
          let (prec, _assoc) = match ident {
            Symbol(name) => prec_map.get(name).unwrap()
            Image(image) =>
              prec_map.get(token_by_image.get(image).unwrap().name).unwrap()
          }
          Some(prec)
        }
        None =>
          // The prec should be determined by the rightmost terminal
          // in the rule. But the items of rule may be modified later, So
          // we will set it to None for now, and delay the decision
          // at building lr1 grammar.
          None
      }
      let clause = Clause::{
        items,
        prec,
        action: elaborate_action(
          items,
          ast_clause_action,
          parser_spec_str~,
          filename~,
          rule_index~,
          clause_index~,
          ast_clause=(ast_clause, ast_clause_action),
          nonterminal_name=rule.name,
          type_=rule.type_,
          json_cst~,
        ),
      }
      rule.clauses.push(clause)
      clause_index += 1
    }
  }
  let mut position_type : TypeExpr = if force_int_position {
    Constr(pkg=None, "Int", [])
  } else {
    Constr(pkg=None, "Unit", [])
  }
  let derive_map = @array_multimap.new()
  for ast_decl in ast_spec.decls {
    match ast_decl {
      Position(type_~) =>
        if not(force_int_position) {
          position_type = parse_and_elaborate_type_expr(type_)
        }
      Derive(traits~, type_~) => {
        let traits = traits
          .split(",")
          .map(t => t.trim(char_set=" ").to_string())
        for trait_ in traits {
          derive_map.add(type_, trait_)
        }
      }
      _ => ()
    }
  }

  // TODO: Make this non target specific
  // JsonCst

  if json_cst is Yes {
    header = @immut/array.new()
    trailer = @immut/array.new()
  }
  ParserSpec::{
    header,
    trailer,
    tokens,
    rules: rule_by_name,
    start_rules,
    position_type,
    derive_map,
  }
}

///|
fn elaborate_action(
  items : Array[Item],
  ast_action : @ast.ClauseAction,
  parser_spec_str~ : String,
  filename~ : String,
  rule_index~ : Int,
  clause_index~ : Int,
  ast_clause~ : (@ast.ClauseWithoutAction, @ast.ClauseAction),
  nonterminal_name~ : String,
  type_~ : TypeExpr,
  json_cst~ : JsonCst,
) -> Action raise ElabError {
  let arity = items.length()
  let name_to_index : Map[String, Int] = {}
  for index, item in items {
    match item.binder {
      None => ()
      Some(name) => name_to_index[name] = index
    }
  }
  fn item_ident_to_index(ident : @ast.ClauseItemIdent) -> Int {
    match ident {
      Dollar(index) => index - 1
      Name(name) => name_to_index.get(name).unwrap()
    }
  }

  let bindings : Array[(BindingSubject, Code)] = []
  let visited = @sorted_set.new()
  fn add_binding(desc : @ast.SubstItemDesc, loc : (Int, Int)) raise ElabError {
    guard not(visited.contains(desc)) else { return }
    visited.add(desc)
    match desc {
      Dollar(index) => {
        let name = "_dollar\{index}"
        guard index - 1 < items.length() else {
          raise OutOfBoundsAccess(index, loc~) // TODO: correct loc
        }
        bindings.push((Data(index - 1, type_=items[index - 1].type_()), name))
      }
      StartPos => bindings.push((StartPos, "_start_pos"))
      Loc => {
        add_binding(StartPos, loc)
        add_binding(EndPos, loc)
      }
      EndPos => bindings.push((EndPos, "_end_pos"))
      StartPosOf(arg) => {
        let index = item_ident_to_index(arg)
        bindings.push((StartPosOf(index), "_start_pos_of_item\{index}"))
      }
      EndPosOf(arg) => {
        let index = item_ident_to_index(arg)
        bindings.push((EndPosOf(index), "_end_pos_of_item\{index}"))
      }
      LocOf(arg) => {
        add_binding(StartPosOf(arg), loc)
        add_binding(EndPosOf(arg), loc)
      }
      SymbolStartPos => bindings.push((SymbolStartPos, "_symbol_start_pos"))
      Sloc => {
        add_binding(SymbolStartPos, loc)
        add_binding(EndPos, loc)
      }
    }
  }

  let body : Array[(Code, (Int, Int)?)] = []
  match json_cst {
    No => {
      for index, item in items {
        match item {
          { binder: Some(name), .. } =>
            bindings.push((Data(index, type_=item.type_()), name))
          _ => ()
        }
      }
      match ast_action.code {
        None => body.push(("()", None))
        Some(code) => {
          let mut last_index = 0
          for item in code.subst {
            if item.start > last_index {
              let len = item.start - last_index
              body.push(
                (
                  try! code.code[last_index:item.start].to_string(),
                  Some((code.utf8_pos + last_index, len)),
                ),
              )
            }
            add_binding(
              item.desc,
              (code.utf8_pos + item.start, code.utf8_pos + item.end),
            )
            body.push(
              (
                match item.desc {
                  Dollar(index) => "_dollar\{index}"
                  StartPos => "_start_pos"
                  EndPos => "_end_pos"
                  Loc => "(_start_pos, _end_pos)"
                  StartPosOf(arg) =>
                    "_start_pos_of_item\{item_ident_to_index(arg)}"
                  EndPosOf(arg) => "_end_pos_of_item\{item_ident_to_index(arg)}"
                  LocOf(arg) => {
                    let index = item_ident_to_index(arg)
                    "(_start_pos_of_item\{index}, _end_pos_of_item\{index})"
                  }
                  SymbolStartPos => "_symbol_start_pos"
                  Sloc => "(_symbol_start_pos, _end_pos)"
                },
                Some((code.utf8_pos + item.start, item.end - item.start)),
              ),
            )
            last_index = item.end
          }
          if last_index < code.code.length() {
            let len = code.code.length() - last_index
            body.push(
              (
                try! code.code[last_index:code.code.length()].to_string(),
                Some((code.utf8_pos + last_index, len)),
              ),
            )
          }
        }
      }
    }
    Yes(_) => {
      add_binding(StartPos, (0, 0)) // TODO: correct loc
      add_binding(EndPos, (0, 0)) // TODO: correct loc
      body.push(
        (
          (
            #|{
            #|  "type": "NONTERMINAL",
            $|  "name": "\{nonterminal_name}",
            $|  "rule_index": \{rule_index},
            $|  "clause_index": \{clause_index},
            $|  "children": args_to_json(_args),
            $|  "start": _start_pos.to_json(),
            $|  "end": _end_pos.to_json(),
            $|}
            $|
          ),
          None,
        ),
      )
    }
  }
  let (ast_clause_without_action, ast_clause_action) = ast_clause
  let (line, column) = offset_to_line_column(
    parser_spec_str,
    ast_clause_without_action.loc.0,
  )
  let original_clause_info = {
    file: filename,
    line: line - 1, // to 0-based
    column: column - 1, // to 0-based
    code: try! parser_spec_str[ast_clause_without_action.loc.0:ast_clause_action.loc.0 +
    ast_clause_action.loc.1].to_string(),
  }
  Action::{
    stamp: @stamp.new(),
    arity,
    type_,
    sub_actions: [],
    bindings,
    body,
    original_clause_info,
  }
}

///|
fn parse_and_elaborate_type_expr(
  str : @ast.TypeString,
) -> TypeExpr raise ElabError {
  let type_expr = @type_expr_parser.parse(str) catch {
    _ => raise FailedToParseTypeExpr(str)
  }
  elaborate_type_expr(type_expr)
}

///|
fn elaborate_type_expr(ast_type_expr : @ast.TypeExpr) -> TypeExpr {
  elaborate_type_expr_with_generic_params(ast_type_expr, Set::new())
}

///|
fn elaborate_type_expr_with_generic_params(
  ast_type_expr : @ast.TypeExpr,
  generic_params : Set[String],
) -> TypeExpr {
  fn elaborate_type_expr(ast_type_expr : @ast.TypeExpr) -> TypeExpr {
    elaborate_type_expr_with_generic_params(ast_type_expr, generic_params)
  }

  match ast_type_expr {
    Constr(pkg=None, name, []) if generic_params.contains(name) => Param(name)
    Constr(pkg~, name, args) =>
      Constr(pkg~, name, args.map(elaborate_type_expr))
    Option(inner) => Option(elaborate_type_expr(inner))
    Tuple(args) => Tuple(args.map(elaborate_type_expr))
    Arrow(args, ret) =>
      Arrow(args.map(elaborate_type_expr), elaborate_type_expr(ret))
  }
}

///|
fn[T, R] map_error(
  arr : Array[T],
  f : (T) -> R raise ElabError,
) -> Array[R] raise ElabError {
  let result : Array[R] = []
  for item in arr {
    result.push(f(item))
  }
  result
}
