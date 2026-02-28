///|
pub fn print_spec(
  spec : ParserSpec,
  out : StringBuilder,
  actions? : Bool = false,
) -> Unit {
  let mut header = None
  let mut trailer = None
  let start_decls = []
  let normal_decls = []
  let prec_decls = []
  for decl in spec.decls {
    match decl {
      Start(_) as decl => start_decls.push(decl)
      Header(code, _, _) => header = Some(code)
      Trailer(code, _, _) => trailer = Some(code)
      Left(_) | Right(_) | Nonassoc(_) as decl => prec_decls.push(decl)
      decl => normal_decls.push(decl)
    }
  }
  for decl in start_decls {
    match decl {
      Start(symbols, type_~) => {
        out.write_string("%start")
        if not(actions) {
          out.write_string("<unit>")
        } else {
          out.write_string("<\{type_}>")
        }
        for symbol in symbols {
          out.write_string(" ")
          out.write_string(symbol.symbol)
        }
        out.write_string("\n")
      }
      _ => panic()
    }
  }
  if not(start_decls.is_empty()) {
    out.write_string("\n")
  }
  for decl in normal_decls {
    match decl {
      Start(_) | Header(_) | Trailer(_) | Left(_) | Right(_) | Nonassoc(_) =>
        panic()
      Token(symbols, type_~) => {
        out.write_string("%token")
        if actions && type_ is Some(type_) {
          out.write_string("<\{type_}>")
        }
        for symbol in symbols {
          out.write_string(" ")
          out.write_string(symbol.symbol)
        }
        out.write_string("\n")
      }
      Token1(symbol, type_~, image~) => {
        out.write_string("%token")
        if actions && type_ is Some(type_) {
          out.write_string("<\{type_}>")
        }
        out.write_string(" ")
        out.write_string(symbol.symbol)
        out.write_string(" ")
        out.write_string(image)
        out.write_string("\n")
      }
      Type(symbols, type_~) =>
        if actions {
          out.write_string("%type")
          out.write_string("<\{type_}>")
          for symbol in symbols {
            out.write_string(" ")
            out.write_string(symbol.symbol)
          }
          out.write_string("\n")
        }
      Derive(traits~, type_~) =>
        if actions {
          out.write_string("%derive")
          out.write_string("<\{type_}>")
          out.write_string(" ")
          out.write_string(traits)
          out.write_string("\n")
        }
      Position(type_~) =>
        if actions {
          out.write_string("%position")
          out.write_string("<\{type_}>")
          out.write_string("\n")
        }
    }
  }
  if not(prec_decls.is_empty()) {
    out.write_string("\n")
  }
  for decl in prec_decls {
    match decl {
      Left(symbols) | Right(symbols) | Nonassoc(symbols) => {
        out.write_string(
          match decl {
            Left(_) => "%left"
            Right(_) => "%right"
            Nonassoc(_) => "%nonassoc"
            _ => panic()
          },
        )
        for symbol in symbols {
          out.write_string(" ")
          match symbol {
            Symbol(symbol) => out.write_string(symbol)
            Image(image) => out.write_string(image)
          }
        }
        out.write_string("\n")
      }
      _ => panic()
    }
  }
  if actions && header is Some(code) {
    out.write_string("\n\n%{\{code}%}\n")
  }
  out.write_string("\n%%\n\n")
  for rule in spec.rules {
    if rule.inline {
      out.write_string("%inline ")
    }
    out.write_string(rule.nonterminal)
    if actions && not(rule.generic_params.is_empty()) {
      out.write_string("[")
      for index, param in rule.generic_params {
        if index > 0 {
          out.write_string(", ")
        }
        out.write_string(param)
      }
      out.write_string("]")
    }
    if not(rule.params.is_empty()) {
      out.write_string("(")
      for index, param in rule.params {
        if index > 0 {
          out.write_string(", ")
        }
        out.write_string(param.0)
        if actions && param.1 is Some(type_) {
          out.write_string(" : \{type_}")
        }
      }
      out.write_string(")")
    }
    if actions && rule.type_ is Some(type_) {
      out.write_string(" -> \{type_}")
    }
    fn term_to_string(term : Term) {
      match term {
        Symbol(symbol, ..) => symbol
        Image(image, ..) => image
        RuleCall(symbol, args, ..) => {
          let args = args.iter().map(term_to_string).join(", ")
          "\{symbol}(\{args})"
        }
      }
    }

    for index, clause in rule.clauses {
      if index == 0 {
        out.write_string("\n  : ")
      } else {
        out.write_string("\n  | ")
      }
      for index, clause_no_action in clause.0 {
        if index > 0 {
          out.write_string("\n | ")
        }
        for index, item in clause_no_action.items {
          if index > 0 {
            out.write_string(" ")
          }
          if actions && item.binder is Some(binder) {
            out.write_string("\{binder}=\{term_to_string(item.term)}")
          } else {
            out.write_string(term_to_string(item.term))
          }
        }
        if clause_no_action.prec is Some(prec) {
          match prec {
            Symbol(symbol) => out.write_string(" %prec \{symbol}")
            Image(image) => out.write_string(" %prec \{image}")
          }
        }
        if not(actions) {
          out.write_string(" {}")
        }
      }
      if actions {
        if clause.1.code is Some(code) {
          out.write_string(" {\{code.code}}")
        } else {
          out.write_string(" {}")
        }
      }
    }
    out.write_string("\n  ;\n\n")
  }
  if actions && trailer is Some(code) {
    out.write_string("%{\{code}%}\n")
  }
}
