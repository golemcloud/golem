// TODO: Move infer_return_type to elaborate.mbt

///|
priv struct DeferredSpecializedRule {
  parametric_rule_name : String
  specialized_rule_name : String
  args : Array[@elab.Term]
  env : @immut/sorted_map.SortedMap[String, @elab.Term]
}

///|
pub fn eliminate_rule_args(
  spec : @elab.ParserSpec,
  json_cst~ : @elab.JsonCst,
) -> @elab.ParserSpec {
  let mut deferred_specialized_rules : Map[String, DeferredSpecializedRule] = {}
  let parametric_rules : Map[String, @elab.Rule] = {}
  let new_rules : Map[String, @elab.Rule] = {}
  fn map_term(
    term : @elab.Term,
    env : @immut/sorted_map.SortedMap[String, @elab.Term],
  ) -> @elab.Term {
    match term {
      Token(_) => term
      RuleCall(rule_name, [], type_) => RuleCall(rule_name, [], type_)
      RuleCall(rule_name, args, _type) => {
        let parametric_rule = spec.rules.get(rule_name).unwrap()
        let args = args.map(fn(arg) { map_term(arg, env) })
        let type_ = infer_return_type(
          parametric_rule,
          args.foldi(init=env, fn(index, env, arg) {
            let param = parametric_rule.params[index]
            env.add(param.0, arg)
          }),
          json_cst~,
        )
        let specialized_rule_name = derive_term_ident(
          RuleCall(rule_name, args, type_),
        )
        deferred_specialized_rules[specialized_rule_name] = {
          parametric_rule_name: rule_name,
          specialized_rule_name,
          args,
          env,
        }
        RuleCall(specialized_rule_name, [], type_)
      }
      Param(name, _type) =>
        match env.get(name) {
          Some(term) => term
          None => term
        }
    }
  }

  fn map_rule(rule : @elab.Rule) -> @elab.Rule {
    {
      ..rule,
      clauses: rule.clauses.map(fn(clause) {
        let items = clause.items.map(fn(item) {
          { ..item, term: map_term(item.term, @immut/sorted_map.new()) }
        })
        {
          ..clause,
          items: clause.items.map(fn(item) {
            { ..item, term: map_term(item.term, @immut/sorted_map.new()) }
          }),
          action: {
            ..clause.action,
            stamp: @stamp.new(),
            bindings: clause.action.bindings.map(fn(binding) {
              let (subject, code) = binding
              let new_subject : @elab.BindingSubject = match subject {
                Data(index, ..) => Data(index, type_=items[index].type_())
                _ => subject
              }
              (new_subject, code)
            }),
          },
        }
      }),
    }
  }

  for rule_name, rule in spec.rules {
    if rule.params.is_empty() {
      new_rules[rule_name] = map_rule(rule)
    } else {
      parametric_rules[rule_name] = rule
    }
  }
  fn specialize_rule(dsr : DeferredSpecializedRule) -> @elab.Rule {
    let { parametric_rule_name, specialized_rule_name, args, env } = dsr
    let parametric_rule = parametric_rules.get(parametric_rule_name).unwrap()
    let mut env : @immut/sorted_map.SortedMap[String, @elab.Term] = env
    for index, param in parametric_rule.params {
      env = env.add(param.0, args[index])
    }
    let type_ = infer_return_type(parametric_rule, env, json_cst~)
    {
      ..parametric_rule,
      name: specialized_rule_name,
      generic_params: Set::new(),
      params: [],
      clauses: parametric_rule.clauses.map(fn(clause) {
        let new_items = clause.items.map(fn(item) {
          { ..item, term: map_term(item.term, env) }
        })
        {
          ..clause,
          items: new_items,
          action: {
            ..clause.action,
            stamp: @stamp.new(),
            bindings: clause.action.bindings.map(fn(binding) {
              let (subject, code) = binding
              let new_subject : @elab.BindingSubject = match subject {
                Data(index, ..) => {
                  let item = new_items[index]
                  Data(index, type_=item.type_())
                }
                _ => subject
              }
              (new_subject, code)
            }),
            type_,
          },
        }
      }),
      type_,
    }
  }

  while true {
    let queue = deferred_specialized_rules
      .values()
      .filter(fn(it) { not(new_rules.contains(it.specialized_rule_name)) })
      .to_array()
    deferred_specialized_rules = {}
    if queue.is_empty() {
      break
    }
    for it in queue {
      new_rules[it.specialized_rule_name] = specialize_rule(it)
    }
  }
  { ..spec, rules: new_rules }
}

///|
fn derive_term_ident(term : @elab.Term) -> String {
  match term {
    Token(token) => token.name
    Param(_) => panic()
    RuleCall(rule_name, [], _type) => rule_name
    RuleCall(rule_name, args, _type) =>
      rule_name + "(" + args.iter().map(derive_term_ident).join(",") + ")"
  }
}

///|
fn infer_return_type(
  parametric_rule : @elab.Rule,
  env : @immut/sorted_map.SortedMap[String, @elab.Term],
  json_cst~ : @elab.JsonCst,
) -> @elab.TypeExpr {
  if json_cst is Yes {
    return Constr(pkg=None, "Json", [])
  }
  let generic_param_map : Map[String, @elab.TypeExpr] = {}
  fn unify(t1 : @elab.TypeExpr, t2 : @elab.TypeExpr) -> Unit {
    match (t1, t2) {
      (Param(name), t2) => generic_param_map[name] = t2
      (Constr(pkg=pkg1, name1, args1), Constr(pkg=pkg2, name2, args2)) => {
        guard pkg1 == pkg2 && name1 == name2 && args1.length() == args2.length() else {
          panic()
        }
        for index, arg1 in args1 {
          unify(arg1, args2[index])
        }
      }
      (Option(t1), Option(t2)) => unify(t1, t2)
      (Tuple(types1), Tuple(types2)) => {
        guard types1.length() == types2.length() else { panic() }
        for index, type1 in types1 {
          unify(type1, types2[index])
        }
      }
      (Arrow(params1, ret1), Arrow(params2, ret2)) => {
        guard params1.length() == params2.length() else { panic() }
        for index, param1 in params1 {
          unify(param1, params2[index])
        }
        unify(ret1, ret2)
      }
      _ => panic()
    }
  }

  for param in parametric_rule.params {
    let arg = env.get(param.0).unwrap()
    unify(
      param.1,
      match arg {
        Token(token) => token.type_
        Param(_, type_) => type_
        RuleCall(_, _, type_) => type_
      },
    )
  }
  fn process_type(type_ : @elab.TypeExpr) -> @elab.TypeExpr {
    match type_ {
      Param(name) => generic_param_map.get(name).unwrap()
      Constr(pkg~, name, args) => Constr(pkg~, name, args.map(process_type))
      Option(type_) => Option(process_type(type_))
      Tuple(types) => Tuple(types.map(process_type))
      Arrow(params, ret) => Arrow(params.map(process_type), process_type(ret))
    }
  }

  process_type(parametric_rule.type_)
}
