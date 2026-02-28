///|
fn find_leaf_inline_rules(rules : Map[String, @elab.Rule]) -> Set[String] {
  let all_inline_rules : Map[String, @elab.Rule] = {}
  for rule_name, rule in rules {
    if rule.inline {
      all_inline_rules[rule_name] = rule
    }
  }
  Set::from_iter(
    all_inline_rules
    .iter()
    .filter(it => {
      let (_, rule) = it
      rule.clauses
      .iter()
      .all(fn(clause) {
        clause.items
        .iter()
        .all(fn(item) {
          match item.term {
            Token(_) => true
            RuleCall(rule_name, [], _type) =>
              not(all_inline_rules.contains(rule_name))
            Param(_) | RuleCall(_, _, _) => panic()
          }
        })
      })
    })
    .map(it => it.0),
  )
}

///|
fn eliminate_inline_rules(
  spec : @elab.ParserSpec,
  inline_rules : Set[String],
) -> @elab.ParserSpec {
  // assert not(inline_rules.is_empty())
  let new_rules : Map[String, @elab.Rule] = {}
  fn contains_inline_rule(items : Array[@elab.Item]) {
    items
    .iter()
    .any(fn(item) {
      match item.term {
        Token(_) => false
        RuleCall(rule_name, [], _type) => inline_rules.contains(rule_name)
        Param(_) | RuleCall(_, _, _) => panic()
      }
    })
  }

  fn expand(clause : @elab.Clause, yield_ : (@elab.Clause) -> Unit) {
    fn cont(
      cont_items : @immut/array.T[@elab.Item],
      cont_bindings : @immut/array.T[(@elab.BindingSubject, @elab.Code)],
      cont_subactions : @immut/array.T[@elab.SubAction],
      index : Int,
    ) {
      if index >= clause.items.length() {
        yield_({
          ..clause,
          items: cont_items.to_array(),
          action: {
            ..clause.action,
            stamp: @stamp.new(),
            sub_actions: cont_subactions.to_array(),
            bindings: cont_bindings.to_array(),
          },
        })
      } else {
        let item = clause.items[index]
        if item.term is RuleCall(rule_name, [], _) &&
          inline_rules.contains(rule_name) {
          let inline_rule = spec.rules.get(rule_name).unwrap()
          for inline_clause in inline_rule.clauses {
            let next_cont_items = cont_items +
              @immut/array.from_array(inline_clause.items)
            let start_index = cont_items.length()
            let end_index = start_index + inline_clause.items.length()
            let subaction_index = cont_subactions.length()
            let next_cont_subactions = cont_subactions.push({
              start: start_index,
              end: end_index,
              action: inline_clause.action,
            })
            let index_shift = inline_clause.items.length() - 1
            let next_cont_bindings = cont_bindings.map(fn(binding) {
              let (subject, code) = binding
              let new_subject : @elab.BindingSubject = match subject {
                Data(data_index, type_~) if data_index == start_index =>
                  SubAction(index=subaction_index, type_~)
                Data(data_index, type_~) if data_index > start_index =>
                  Data(data_index + index_shift, type_~)
                StartPosOf(data_index) if data_index > start_index =>
                  StartPosOf(data_index + index_shift)
                EndPosOf(data_index) if data_index > start_index =>
                  EndPosOf(data_index + index_shift)
                _ => subject
              }
              (new_subject, code)
            })
            cont(
              next_cont_items,
              next_cont_bindings,
              next_cont_subactions,
              index + 1,
            )
          }
        } else {
          cont(cont_items.push(item), cont_bindings, cont_subactions, index + 1)
        }
      }
    }

    cont(
      @immut/array.new(),
      @immut/array.from_array(clause.action.bindings),
      @immut/array.from_array(clause.action.sub_actions),
      0,
    )
  }

  for rule_name, rule in spec.rules {
    guard not(inline_rules.contains(rule_name)) else { continue }
    let new_clauses : Array[@elab.Clause] = []
    for clause in rule.clauses {
      if contains_inline_rule(clause.items) {
        expand(clause, fn(clause) { new_clauses.push(clause) })
      } else {
        new_clauses.push(clause)
      }
    }
    let new_rule = { ..rule, clauses: new_clauses }
    new_rules[rule_name] = new_rule
  }
  { ..spec, rules: new_rules }
}

///|
pub fn eliminate_inline(spec : @elab.ParserSpec) -> @elab.ParserSpec {
  let mut spec = spec
  while true {
    let inline_rules = find_leaf_inline_rules(spec.rules)
    if inline_rules.is_empty() {
      break
    } else {
      spec = eliminate_inline_rules(spec, inline_rules)
    }
  }
  spec
}
