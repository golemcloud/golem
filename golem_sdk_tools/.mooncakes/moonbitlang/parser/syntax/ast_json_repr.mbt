///|
fn[T] list_json_repr(
  kind : String,
  loc : Location,
  list : @list.List[T],
  f : (T) -> Json,
) -> Json {
  let children = []
  list.each(fn(item) { children.push(f(item)) })
  { "kind": kind, "loc": loc, "children": children }
}

///|
fn[T] option_json_repr(opt : T?, f : (T) -> Json) -> Json {
  match opt {
    Some(value) => f(value)
    None => Json::null()
  }
}

///|
fn tagged_node(
  kind : String,
  loc : Location,
  children : Map[String, Json],
) -> Json {
  { "kind": kind, "loc": loc, "children": children }
}

///|
fn tagged_node1(kind : String, loc : Location, value : Json) -> Json {
  tagged_node(kind, loc, { "value": value })
}

///|
fn tagged_node0(kind : String, loc : Location) -> Json {
  tagged_node(kind, loc, {})
}

///|
fn string_literal_json_repr(s : StringLiteral) -> Json {
  Json::string(s)
}

///|
fn char_literal_json_repr(c : CharLiteral) -> Json {
  Json::string(c)
}

///|
fn byte_literal_json_repr(b : ByteLiteral) -> Json {
  Json::string(b)
}

///|
fn bytes_literal_json_repr(b : BytesLiteral) -> Json {
  Json::string(b)
}

///|
pub fn LongIdent::json_repr(self : LongIdent, loc : Location) -> Json {
  match self {
    Ident(name~) => tagged_node1("LongIdent::Ident", loc, Json::string(name))
    Dot(pkg~, id~) =>
      tagged_node("LongIdent::Dot", loc, {
        "pkg": Json::string(pkg),
        "id": Json::string(id),
      })
  }
}

///|
pub fn ConstrId::json_repr(self : ConstrId) -> Json {
  tagged_node("ConstrId", self.loc, { "id": self.id.json_repr(self.loc) })
}

///|
pub fn FuncStubs::json_repr(self : FuncStubs, loc : Location) -> Json {
  match self {
    Import(module_name~, func_name~, language~) =>
      tagged_node("FuncStubs::Import", loc, {
        "module_name": string_literal_json_repr(module_name),
        "func_name": string_literal_json_repr(func_name),
        "language": option_json_repr(language, string_literal_json_repr),
      })
    Embedded(language~, code~) =>
      tagged_node("FuncStubs::Embedded", loc, {
        "language": option_json_repr(language, string_literal_json_repr),
        "code": code.json_repr(loc),
      })
  }
}

///|
pub fn EmbeddedCode::json_repr(self : EmbeddedCode, loc : Location) -> Json {
  match self {
    CodeString(s) =>
      tagged_node1("EmbeddedCode::CodeString", loc, string_literal_json_repr(s))
    CodeMultilineString(strings) =>
      tagged_node1(
        "EmbeddedCode::CodeMultilineString",
        loc,
        list_json_repr(
          "EmbeddedCode::CodeMultilineString::LiteralList",
          loc,
          strings,
          Json::string,
        ),
      )
  }
}

///|
pub fn TraitMethodDecl::json_repr(self : TraitMethodDecl) -> Json {
  tagged_node("TraitMethodDecl", self.loc, {
    "name": self.name.json_repr(),
    "has_error": option_json_repr(self.has_error, raise_json_repr),
    "is_async": option_json_repr(self.is_async, async_json_repr),
    "quantifiers": list_json_repr(
      "TraitMethodDecl::QuantifierList",
      self.loc,
      self.quantifiers,
      TypeVarBinder::json_repr,
    ),
    "params": list_json_repr(
      "TraitMethodDecl::ParamList",
      self.loc,
      self.params,
      Parameter::json_repr,
    ),
    "return_type": option_json_repr(self.return_type, Type::json_repr),
    "error_type": self.error_type.json_repr(self.loc),
    "has_default": option_json_repr(self.has_default, fn(loc) {
      tagged_node0("TraitMethodDecl::HasDefault", loc)
    }),
    "attrs": list_json_repr(
      "TraitMethodDecl::AttrList",
      self.loc,
      self.attrs,
      Attribute::json_repr,
    ),
  })
}

///|
pub fn TypeVarConstraint::json_repr(self : TypeVarConstraint) -> Json {
  tagged_node("TypeVarConstraint", self.loc, {
    "trait": self.trait_.json_repr(self.loc),
  })
}

///|
pub fn TypeDeclBinder::json_repr(self : TypeDeclBinder) -> Json {
  tagged_node("TypeDeclBinder", self.loc, {
    "name": option_json_repr(self.name, Json::string),
  })
}

///|
pub fn TypeDesc::json_repr(self : TypeDesc, loc : Location) -> Json {
  match self {
    Abstract => tagged_node0("TypeDesc::Abstract", loc)
    Extern => tagged_node0("TypeDesc::Extern", loc)
    Newtype(ty) => tagged_node1("TypeDesc::Newtype", ty.loc(), ty.json_repr())
    Error(exception_decl) =>
      tagged_node1("TypeDesc::Error", loc, exception_decl.json_repr(loc))
    Variant(constrs) => {
      let constrs_loc = match constrs {
        @list.Empty => loc
        @list.More(constr, tail=@list.Empty) => constr.loc
        @list.More(first, tail~) => first.loc.merge(tail.last().unwrap().loc)
      }
      tagged_node1(
        "TypeDesc::Variant",
        constrs_loc,
        list_json_repr(
          "TypeDesc::Variant::ConstrList",
          constrs_loc,
          constrs,
          ConstrDecl::json_repr,
        ),
      )
    }
    Record(fields) =>
      tagged_node1(
        "TypeDesc::Record",
        loc,
        list_json_repr(
          "TypeDesc::Record::FieldList",
          loc,
          fields,
          FieldDecl::json_repr,
        ),
      )
    TupleStruct(tys) => {
      let tys_loc = match tys {
        @list.Empty => loc
        @list.More(ty, tail=@list.Empty) => ty.loc()
        @list.More(first, tail~) =>
          first.loc().merge(tail.last().unwrap().loc())
      }
      tagged_node1(
        "TypeDesc::TupleStruct",
        tys_loc,
        list_json_repr(
          "TypeDesc::TupleStruct::TypeList",
          tys_loc,
          tys,
          Type::json_repr,
        ),
      )
    }
    Alias(ty) => tagged_node1("TypeDesc::Alias", ty.loc(), ty.json_repr())
  }
}

///|
pub fn DerivingDirective::json_repr(self : DerivingDirective) -> Json {
  tagged_node("DerivingDirective", self.loc, {
    "type_name": self.type_name.json_repr(),
    "args": list_json_repr(
      "DerivingDirective::ArgList",
      self.loc,
      self.args,
      Argument::json_repr,
    ),
  })
}

///|
pub fn ExceptionDecl::json_repr(self : ExceptionDecl, loc : Location) -> Json {
  match self {
    NoPayload => tagged_node0("ExceptionDecl::NoPayload", loc)
    SinglePayload(ty) =>
      tagged_node1("ExceptionDecl::SinglePayload", ty.loc(), ty.json_repr())
    EnumPayload(constrs) =>
      tagged_node1(
        "ExceptionDecl::EnumPayload",
        loc,
        list_json_repr(
          "ExceptionDecl::EnumPayload::ConstrList",
          loc,
          constrs,
          ConstrDecl::json_repr,
        ),
      )
  }
}

///|
pub fn ConstrDecl::json_repr(self : ConstrDecl) -> Json {
  tagged_node("ConstrDecl", self.loc, {
    "name": self.name.json_repr(),
    "args": option_json_repr(self.args, fn(args) {
      list_json_repr(
        "ConstrDecl::ArgList",
        self.loc,
        args,
        ConstrParam::json_repr,
      )
    }),
    "tag": option_json_repr(self.tag, fn(tag) {
      let (s, l) = tag
      tagged_node1("ConstrDecl::Tag", l, Json::string(s))
    }),
    "attrs": list_json_repr(
      "ConstrDecl::AttrList",
      self.loc,
      self.attrs,
      Attribute::json_repr,
    ),
    "doc": self.doc,
  })
}

///|
pub fn ConstrName::json_repr(self : ConstrName) -> Json {
  tagged_node("ConstrName", self.loc, { "name": Json::string(self.name) })
}

///|
pub fn ConstrParam::json_repr(self : ConstrParam) -> Json {
  let loc = match self.label {
    Some(label) => label.loc.merge(self.ty.loc())
    None => self.ty.loc()
  }
  tagged_node("ConstrParam", loc, {
    "ty": self.ty.json_repr(),
    "mut": Json::boolean(self.mut_),
    "label": option_json_repr(self.label, Label::json_repr),
  })
}

///|
pub fn FieldDecl::json_repr(self : FieldDecl) -> Json {
  tagged_node("FieldDecl", self.loc, {
    "name": self.name.json_repr(),
    "ty": self.ty.json_repr(),
    "mut": Json::boolean(self.mut_),
    "vis": self.vis.json_repr(self.loc),
    "attrs": list_json_repr(
      "FieldDecl::AttrList",
      self.loc,
      self.attrs,
      Attribute::json_repr,
    ),
    "doc": self.doc,
  })
}

///|
pub fn FieldName::json_repr(self : FieldName) -> Json {
  tagged_node("FieldName", self.loc, { "label": Json::string(self.label) })
}

///|
pub fn WhereClause::json_repr(self : WhereClause) -> Json {
  let { fields, loc } = self
  tagged_node("WhereClause", loc, {
    "fields": list_json_repr(
      "WhereClause::FieldList",
      loc,
      fields,
      FieldDef::json_repr,
    ),
  })
}

///|
pub fn Expr::json_repr(self : Expr) -> Json {
  match self {
    Apply(func~, args~, attr~, loc~) => {
      let args_list = list_json_repr(
        "Expr::Apply::ArgumentList",
        loc,
        args,
        Argument::json_repr,
      )
      tagged_node("Expr::Apply", loc, {
        "func": func.json_repr(),
        "args": args_list,
        "attr": attr.json_repr(loc),
      })
    }
    Infix(op~, lhs~, rhs~, loc~) =>
      tagged_node("Expr::Infix", loc, {
        "op": op.json_repr(),
        "lhs": lhs.json_repr(),
        "rhs": rhs.json_repr(),
      })
    Unary(op~, expr~, loc~) =>
      tagged_node("Expr::Unary", loc, {
        "op": op.json_repr(),
        "expr": expr.json_repr(),
      })
    Array(exprs~, loc~) =>
      tagged_node("Expr::Array", loc, {
        "exprs": list_json_repr(
          "Expr::Array::ExprList",
          loc,
          exprs,
          Expr::json_repr,
        ),
      })
    ArraySpread(elems~, loc~) =>
      tagged_node("Expr::ArraySpread", loc, {
        "elems": list_json_repr(
          "Expr::ArraySpread::ElemList",
          loc,
          elems,
          SpreadableElem::json_repr,
        ),
      })
    ArrayGet(array~, index~, loc~) =>
      tagged_node("Expr::ArrayGet", loc, {
        "array": array.json_repr(),
        "index": index.json_repr(),
      })
    ArrayGetSlice(array~, start_index~, end_index~, index_loc~, loc~) => {
      ignore(index_loc)
      tagged_node("Expr::ArrayGetSlice", loc, {
        "array": array.json_repr(),
        "start_index": option_json_repr(start_index, Expr::json_repr),
        "end_index": option_json_repr(end_index, Expr::json_repr),
      })
    }
    ArraySet(array~, index~, value~, loc~) =>
      tagged_node("Expr::ArraySet", loc, {
        "array": array.json_repr(),
        "index": index.json_repr(),
        "value": value.json_repr(),
      })
    ArrayAugmentedSet(op~, array~, index~, value~, loc~) =>
      tagged_node("Expr::ArrayAugmentedSet", loc, {
        "op": op.json_repr(),
        "array": array.json_repr(),
        "index": index.json_repr(),
        "value": value.json_repr(),
      })
    Constant(c~, loc~) =>
      tagged_node("Expr::Constant", loc, { "constant": c.json_repr(loc) })
    MultilineString(elems~, loc~) =>
      tagged_node("Expr::MultilineString", loc, {
        "elems": list_json_repr("Expr::MultilineString::ElemList", loc, elems, fn(
          elem,
        ) {
          elem.json_repr(loc)
        }),
      })
    Interp(elems~, loc~) =>
      tagged_node("Expr::Interp", loc, {
        "elems": list_json_repr(
          "Expr::Interp::ElemList",
          loc,
          elems,
          InterpElem::json_repr,
        ),
      })
    Constraint(expr~, ty~, loc~) =>
      tagged_node("Expr::Constraint", loc, {
        "expr": expr.json_repr(),
        "ty": ty.json_repr(),
      })
    Constr(constr~, loc~) =>
      tagged_node("Expr::Constr", loc, { "constr": constr.json_repr() })
    While(loop_cond~, loop_body~, while_else~, label~, loc~) =>
      tagged_node("Expr::While", loc, {
        "loop_cond": loop_cond.json_repr(),
        "loop_body": loop_body.json_repr(),
        "while_else": option_json_repr(while_else, Expr::json_repr),
        "label": option_json_repr(label, Label::json_repr),
      })
    Function(func~, loc~) =>
      tagged_node("Expr::Function", loc, { "func": func.json_repr() })
    Ident(id~, loc~) =>
      tagged_node("Expr::Ident", loc, { "id": id.json_repr() })
    If(cond~, ifso~, ifnot~, loc~) =>
      tagged_node("Expr::If", loc, {
        "cond": cond.json_repr(),
        "ifso": ifso.json_repr(),
        "ifnot": option_json_repr(ifnot, Expr::json_repr),
      })
    Guard(cond~, otherwise~, body~, loc~) =>
      tagged_node("Expr::Guard", loc, {
        "cond": cond.json_repr(),
        "otherwise": option_json_repr(otherwise, Expr::json_repr),
        "body": body.json_repr(),
      })
    Is(expr~, pat~, loc~) =>
      tagged_node("Expr::Is", loc, {
        "expr": expr.json_repr(),
        "pat": pat.json_repr(),
      })
    IsLexMatch(expr~, strategy~, pat~, pat_loc~, loc~) =>
      tagged_node("Expr::IsLexmatch", loc, {
        "expr": expr.json_repr(),
        "strategy": option_json_repr(strategy, Label::json_repr),
        "pat": list_json_repr("Expr::IsLexmatch::PatternList", pat_loc, pat, fn(
          p,
        ) {
          p.json_repr(pat_loc)
        }),
      })
    Defer(expr~, body~, loc~) =>
      tagged_node("Expr::Defer", loc, {
        "expr": expr.json_repr(),
        "body": body.json_repr(),
      })
    LetFn(name~, func~, body~, loc~) =>
      tagged_node("Expr::LetFn", loc, {
        "name": name.json_repr(),
        "func": func.json_repr(),
        "body": body.json_repr(),
      })
    LetRec(bindings~, body~, loc~) =>
      tagged_node("Expr::LetRec", loc, {
        "bindings": list_json_repr("Expr::LetRec::BindingList", loc, bindings, fn(
          binding,
        ) {
          let (binder, func) = binding
          tagged_node("LetRec::Binding", loc, {
            "binder": binder.json_repr(),
            "func": func.json_repr(),
          })
        }),
        "body": body.json_repr(),
      })
    LetAnd(bindings~, body~, loc~) =>
      tagged_node("Expr::LetAnd", loc, {
        "bindings": list_json_repr("Expr::LetAnd::BindingList", loc, bindings, fn(
          binding,
        ) {
          let (binder, ty, func) = binding
          tagged_node("LetAnd::Binding", loc, {
            "binder": binder.json_repr(),
            "type": option_json_repr(ty, Type::json_repr),
            "func": func.json_repr(),
          })
        }),
        "body": body.json_repr(),
      })
    Let(pattern~, expr~, body~, loc~) =>
      tagged_node("Expr::Let", loc, {
        "pattern": pattern.json_repr(),
        "expr": expr.json_repr(),
        "body": body.json_repr(),
      })
    Sequence(exprs~, last_expr~, loc~) =>
      tagged_node("Expr::Sequence", loc, {
        "exprs": list_json_repr(
          "Expr::Sequence::ExprList",
          loc,
          exprs,
          Expr::json_repr,
        ),
        "last_expr": last_expr.json_repr(),
      })
    Tuple(exprs~, loc~) =>
      tagged_node("Expr::Tuple", loc, {
        "exprs": list_json_repr(
          "Expr::Tuple::ExprList",
          loc,
          exprs,
          Expr::json_repr,
        ),
      })
    Record(type_name~, fields~, trailing~, loc~) =>
      tagged_node("Expr::Record", loc, {
        "type_name": option_json_repr(type_name, TypeName::json_repr),
        "fields": list_json_repr(
          "Expr::Record::FieldList",
          loc,
          fields,
          FieldDef::json_repr,
        ),
        "trailing": trailing.json_repr(loc),
      })
    RecordUpdate(type_name~, record~, fields~, loc~) =>
      tagged_node("Expr::RecordUpdate", loc, {
        "type_name": option_json_repr(type_name, TypeName::json_repr),
        "record": record.json_repr(),
        "fields": list_json_repr(
          "Expr::RecordUpdate::FieldList",
          loc,
          fields,
          FieldDef::json_repr,
        ),
      })
    Field(record~, accessor~, loc~) =>
      tagged_node("Expr::Field", loc, {
        "record": record.json_repr(),
        "accessor": accessor.json_repr(),
      })
    Method(type_name~, method_name~, loc~) =>
      tagged_node("Expr::Method", loc, {
        "type_name": type_name.json_repr(),
        "method_name": method_name.json_repr(),
      })
    DotApply(self~, method_name~, args~, return_self~, attr~, loc~) => {
      let args_list = list_json_repr(
        "Expr::DotApply::ArgList",
        loc,
        args,
        Argument::json_repr,
      )
      tagged_node("Expr::DotApply", loc, {
        "self": self.json_repr(),
        "method_name": method_name.json_repr(),
        "args": args_list,
        "return_self": Json::boolean(return_self),
        "attr": attr.json_repr(loc),
      })
    }
    As(expr~, trait_~, loc~) =>
      tagged_node("Expr::As", loc, {
        "expr": expr.json_repr(),
        "trait": trait_.json_repr(),
      })
    Mutate(record~, accessor~, field~, augmented_by~, loc~) =>
      tagged_node("Expr::Mutate", loc, {
        "record": record.json_repr(),
        "accessor": accessor.json_repr(),
        "field": field.json_repr(),
        "augmented_by": option_json_repr(augmented_by, Var::json_repr),
      })
    Match(expr~, cases~, match_loc~, loc~) => {
      ignore(match_loc)
      tagged_node("Expr::Match", loc, {
        "expr": expr.json_repr(),
        "cases": list_json_repr(
          "Expr::Match::CaseList",
          loc,
          cases,
          Case::json_repr,
        ),
      })
    }
    LexMatch(strategy~, expr~, match_loc~, cases~, loc~) => {
      ignore(match_loc)
      tagged_node("Expr::Lexmatch", loc, {
        "strategy": option_json_repr(strategy, Label::json_repr),
        "expr": expr.json_repr(),
        "cases": list_json_repr("Expr::Lexmatch::CaseList", loc, cases, fn(
          case,
        ) {
          case.json_repr(loc)
        }),
      })
    }
    LetMut(binder~, ty~, expr~, body~, loc~) =>
      tagged_node("Expr::LetMut", loc, {
        "binder": binder.json_repr(),
        "ty": option_json_repr(ty, Type::json_repr),
        "expr": expr.json_repr(),
        "body": body.json_repr(),
      })
    Pipe(lhs~, rhs~, loc~) =>
      tagged_node("Expr::Pipe", loc, {
        "lhs": lhs.json_repr(),
        "rhs": rhs.json_repr(),
      })
    Assign(var_~, expr~, augmented_by~, loc~) =>
      tagged_node("Expr::Assign", loc, {
        "var": var_.json_repr(),
        "expr": expr.json_repr(),
        "augmented_by": option_json_repr(augmented_by, Var::json_repr),
      })
    Hole(loc~, kind~) =>
      tagged_node("Expr::Hole", loc, { "kind": kind.json_repr(loc) })
    Return(return_value~, loc~) =>
      tagged_node("Expr::Return", loc, {
        "return_value": option_json_repr(return_value, Expr::json_repr),
      })
    Raise(err_value~, loc~) =>
      tagged_node("Expr::Raise", loc, { "err_value": err_value.json_repr() })
    Unit(loc~, faked~) =>
      tagged_node("Expr::Unit", loc, { "faked": Json::boolean(faked) })
    Break(arg~, label~, loc~) =>
      tagged_node("Expr::Break", loc, {
        "arg": option_json_repr(arg, Expr::json_repr),
        "label": option_json_repr(label, Label::json_repr),
      })
    Continue(args~, label~, loc~) =>
      tagged_node("Expr::Continue", loc, {
        "args": list_json_repr(
          "Expr::Continue::ExprList",
          loc,
          args,
          Expr::json_repr,
        ),
        "label": option_json_repr(label, Label::json_repr),
      })
    Loop(arg~, body~, label~, loop_loc~, loc~) => {
      ignore(loop_loc)
      tagged_node("Expr::Loop", loc, {
        "arg": arg.json_repr(),
        "body": list_json_repr(
          "Expr::Loop::CaseList",
          loc,
          body,
          Case::json_repr,
        ),
        "label": option_json_repr(label, Label::json_repr),
      })
    }
    For(
      binders~,
      condition~,
      continue_block~,
      body~,
      for_else~,
      label~,
      loc~,
      where_clause~
    ) =>
      tagged_node("Expr::For", loc, {
        "binders": list_json_repr("Expr::For::BindingList", loc, binders, fn(
          binding,
        ) {
          let (binder, expr) = binding
          tagged_node("For::Binding", loc, {
            "binder": binder.json_repr(),
            "expr": expr.json_repr(),
          })
        }),
        "condition": option_json_repr(condition, Expr::json_repr),
        "continue_block": list_json_repr(
          "Expr::For::ContBindingList",
          loc,
          continue_block,
          fn(binding) {
            let (binder, expr) = binding
            tagged_node("For::ContBinding", loc, {
              "binder": binder.json_repr(),
              "expr": expr.json_repr(),
            })
          },
        ),
        "body": body.json_repr(),
        "for_else": option_json_repr(for_else, Expr::json_repr),
        "where_clause": option_json_repr(where_clause, WhereClause::json_repr),
        "label": option_json_repr(label, Label::json_repr),
      })
    ForEach(binders~, expr~, body~, else_block~, label~, loc~) =>
      tagged_node("Expr::ForEach", loc, {
        "binders": list_json_repr("Expr::ForEach::BinderList", loc, binders, fn(
          b,
        ) {
          option_json_repr(b, Binder::json_repr)
        }),
        "expr": expr.json_repr(),
        "body": body.json_repr(),
        "else_block": option_json_repr(else_block, Expr::json_repr),
        "label": option_json_repr(label, Label::json_repr),
      })
    Try(
      body~,
      catch_~,
      catch_all~,
      try_else~,
      has_try~,
      try_loc~,
      catch_loc~,
      else_loc~,
      loc~
    ) => {
      ignore(try_loc)
      ignore(catch_loc)
      tagged_node("Expr::Try", loc, {
        "body": body.json_repr(),
        "catch": list_json_repr(
          "Expr::Try::CaseList",
          loc,
          catch_,
          Case::json_repr,
        ),
        "catch_all": Json::boolean(catch_all),
        "try_else": option_json_repr(try_else, fn(cases) {
          list_json_repr(
            "Expr::Try::CaseList",
            else_loc,
            cases,
            Case::json_repr,
          )
        }),
        "has_try": Json::boolean(has_try),
      })
    }
    TryOperator(body~, kind~, try_loc~, loc~) => {
      ignore(try_loc)
      tagged_node("Expr::TryOperator", loc, {
        "body": body.json_repr(),
        "kind": kind.json_repr(loc),
      })
    }
    Map(elems~, loc~) =>
      tagged_node("Expr::Map", loc, {
        "elems": list_json_repr(
          "Expr::Map::ElemList",
          loc,
          elems,
          MapExprElem::json_repr,
        ),
      })
    Group(expr~, group~, loc~) =>
      tagged_node("Expr::Group", loc, {
        "expr": expr.json_repr(),
        "group": group.json_repr(loc),
      })
    StaticAssert(asserts~, body~) => {
      let loc = body.loc()
      tagged_node("Expr::StaticAssert", loc, {
        "asserts": list_json_repr(
          "Expr::StaticAssert::StaticAssertionList",
          loc,
          asserts,
          StaticAssertion::json_repr,
        ),
        "body": body.json_repr(),
      })
    }
  }
}

///|
pub fn LocalTypeDecl::json_repr(self : LocalTypeDecl) -> Json {
  tagged_node("LocalTypeDecl", self.tycon_loc, {
    "tycon": Json::string(self.tycon),
    "components": self.components.json_repr(self.tycon_loc),
    "deriving": list_json_repr(
      "LocalTypeDecl::DerivingList",
      self.tycon_loc,
      self.deriving,
      DerivingDirective::json_repr,
    ),
  })
}

///|
pub fn Parameter::json_repr(self : Parameter) -> Json {
  match self {
    DiscardPositional(ty~, loc~) =>
      tagged_node("Parameter::DiscardPositional", loc, {
        "ty": option_json_repr(ty, Type::json_repr),
      })
    Positional(binder~, ty~) => {
      let loc = match ty {
        Some(ty) => binder.loc.merge(ty.loc())
        None => binder.loc
      }
      tagged_node("Parameter::Positional", loc, {
        "binder": binder.json_repr(),
        "ty": option_json_repr(ty, Type::json_repr),
      })
    }
    Labelled(binder~, ty~) => {
      let loc = match ty {
        Some(ty) => binder.loc.merge(ty.loc())
        None => binder.loc
      }
      tagged_node("Parameter::Labelled", loc, {
        "binder": binder.json_repr(),
        "ty": option_json_repr(ty, Type::json_repr),
      })
    }
    Optional(binder~, default~, ty~) => {
      let loc = binder.loc.merge(default.loc())
      tagged_node("Parameter::Optional", loc, {
        "binder": binder.json_repr(),
        "default": default.json_repr(),
        "ty": option_json_repr(ty, Type::json_repr),
      })
    }
    QuestionOptional(binder~, ty~) => {
      let loc = match ty {
        Some(ty) => binder.loc.merge(ty.loc())
        None => binder.loc
      }
      tagged_node("Parameter::QuestionOptional", loc, {
        "binder": binder.json_repr(),
        "ty": option_json_repr(ty, Type::json_repr),
      })
    }
  }
}

///|
fn TestName::json_repr(self : TestName) -> Json {
  option_json_repr(self, fn(test_name) {
    let (s, loc) = test_name
    tagged_node1("TestName", loc, string_literal_json_repr(s))
  })
}

///|
fn Attribute::json_repr(self : Attribute) -> Json {
  tagged_node("Attribute", self.loc, { "raw": Json::string(self.raw) })
}

///|
pub fn TypeDecl::json_repr(self : TypeDecl) -> Json {
  tagged_node("TypeDecl", self.loc, {
    "tycon": Json::string(self.tycon),
    "params": list_json_repr(
      "TypeDecl::ParamList",
      self.loc,
      self.params,
      TypeDeclBinder::json_repr,
    ),
    "components": self.components.json_repr(self.loc),
    "attrs": list_json_repr(
      "TypeDecl::AttrList",
      self.loc,
      self.attrs,
      Attribute::json_repr,
    ),
    "doc": self.doc,
    "type_vis": self.type_vis.json_repr(self.loc),
    "is_declare": self.is_declare,
    "deriving": list_json_repr(
      "TypeDecl::DerivingList",
      self.loc,
      self.deriving,
      DerivingDirective::json_repr,
    ),
  })
}

///|
pub fn FunDecl::json_repr(self : FunDecl, loc : Location) -> Json {
  tagged_node("FunDecl", loc, {
    "type_name": option_json_repr(self.type_name, TypeName::json_repr),
    "name": self.name.json_repr(),
    "has_error": option_json_repr(self.has_error, raise_json_repr),
    "is_async": option_json_repr(self.is_async, async_json_repr),
    "decl_params": option_json_repr(self.decl_params, fn(params) {
      list_json_repr(
        "FunDecl::ParameterList",
        self.params_loc,
        params,
        Parameter::json_repr,
      )
    }),
    "quantifiers": list_json_repr(
      "FunDecl::QuantifierList",
      self.params_loc,
      self.quantifiers,
      TypeVarBinder::json_repr,
    ),
    "return_type": option_json_repr(self.return_type, Type::json_repr),
    "error_type": self.error_type.json_repr(loc),
    "vis": self.vis.json_repr(self.params_loc),
    "attrs": list_json_repr(
      "FunDecl::AttrList",
      self.params_loc,
      self.attrs,
      Attribute::json_repr,
    ),
    "doc": self.doc,
  })
}

///|
pub fn DeclBody::json_repr(self : DeclBody, loc : Location) -> Json {
  match self {
    DeclBody(local_types~, expr~) =>
      tagged_node("DeclBody::DeclBody", loc, {
        "local_types": list_json_repr(
          "DeclBody::DeclBody::LocalTypeList",
          loc,
          local_types,
          LocalTypeDecl::json_repr,
        ),
        "expr": expr.json_repr(),
      })
    DeclStubs(stubs) =>
      tagged_node1("DeclBody::DeclStubs", loc, stubs.json_repr(loc))
    DeclNone => tagged_node0("DeclBody::DeclNone", loc)
  }
}

///|
pub fn Label::json_repr(self : Label) -> Json {
  tagged_node("Label", self.loc, { "name": Json::string(self.name) })
}

///|
pub fn AliasTarget::json_repr(self : AliasTarget) -> Json {
  tagged_node("AliasTarget", self.loc(), {
    "binder": self.binder.json_repr(),
    "target": option_json_repr(self.target, Label::json_repr),
  })
}

///|
pub fn Binder::json_repr(self : Binder) -> Json {
  tagged_node("Binder", self.loc, { "name": Json::string(self.name) })
}

///|
pub fn Type::json_repr(self : Type) -> Json {
  match self {
    Any(loc~) => tagged_node0("Type::Any", loc)
    Arrow(args~, res~, err~, is_async~, loc~) =>
      tagged_node("Type::Arrow", loc, {
        "args": list_json_repr(
          "Type::Arrow::ArgList",
          loc,
          args,
          Type::json_repr,
        ),
        "res": res.json_repr(),
        "err": err.json_repr(loc),
        "is_async": option_json_repr(is_async, async_json_repr),
      })
    Tuple(tys~, loc~) =>
      tagged_node("Type::Tuple", loc, {
        "tys": list_json_repr(
          "Type::Tuple::TypeList",
          loc,
          tys,
          Type::json_repr,
        ),
      })
    Name(constr_id~, tys~, loc~) =>
      tagged_node("Type::Name", loc, {
        "constr_id": constr_id.json_repr(),
        "tys": list_json_repr("Type::Name::TypeList", loc, tys, Type::json_repr),
      })
    Option(ty~, loc~, question_loc~) => {
      ignore(question_loc)
      tagged_node("Type::Option", loc, { "ty": ty.json_repr() })
    }
    Object(constr_id) =>
      tagged_node1("Type::Object", constr_id.loc, constr_id.json_repr())
  }
}

///|
pub fn Visibility::json_repr(self : Visibility, loc : Location) -> Json {
  match self {
    Default => tagged_node0("Visibility::Default", loc)
    Priv(loc~) => tagged_node0("Visibility::Priv", loc)
    Pub(attr~, loc~) =>
      tagged_node("Visibility::Pub", loc, {
        "attr": option_json_repr(attr, Json::string),
      })
  }
}

///|
pub fn TraitDecl::json_repr(self : TraitDecl) -> Json {
  tagged_node("TraitDecl", self.loc, {
    "name": self.name.json_repr(),
    "supers": list_json_repr(
      "TraitDecl::SuperList",
      self.loc,
      self.supers,
      TypeVarConstraint::json_repr,
    ),
    "methods": list_json_repr(
      "TraitDecl::MethodList",
      self.loc,
      self.methods,
      TraitMethodDecl::json_repr,
    ),
    "vis": self.vis.json_repr(self.loc),
    "is_declare": self.is_declare,
    "attrs": list_json_repr(
      "TraitDecl::AttrList",
      self.loc,
      self.attrs,
      Attribute::json_repr,
    ),
    "doc": self.doc,
  })
}

///|
pub fn TypeName::json_repr(self : TypeName) -> Json {
  tagged_node("TypeName", self.loc, {
    "name": self.name.json_repr(self.loc),
    "is_object": Json::boolean(self.is_object),
  })
}

///|
pub fn TypeVarBinder::json_repr(self : TypeVarBinder) -> Json {
  let constraints_loc = match self.constraints {
    @list.Empty => self.name_loc
    @list.More(constraint_, tail=@list.Empty) => constraint_.loc
    @list.More(first, tail~) => first.loc.merge(tail.last().unwrap().loc)
  }
  let loc = self.name_loc.merge(constraints_loc)
  tagged_node("TypeVarBinder", loc, {
    "name": Json::string(self.name),
    "constraints": list_json_repr(
      "TypeVarBinder::TypeVarConstraintList",
      constraints_loc,
      self.constraints,
      TypeVarConstraint::json_repr,
    ),
  })
}

///|
pub fn ErrorType::json_repr(self : ErrorType, loc : Location) -> Json {
  match self {
    ErrorType(ty~) =>
      tagged_node("ErrorType::ErrorType", ty.loc(), { "ty": ty.json_repr() })
    DefaultErrorType(loc~) => tagged_node0("ErrorType::DefaultErrorType", loc)
    NoErrorType => tagged_node0("ErrorType::NoErrorType", loc)
    Noraise(loc~) => tagged_node0("ErrorType::Noraise", loc)
    MaybeError(ty~) =>
      tagged_node("ErrorType::MaybeError", ty.loc(), { "ty": ty.json_repr() })
  }
}

///|
fn using_name_json_repr(item : (AliasTarget, UsingKind)) -> Json {
  let (target, kind) = item
  let loc = target.loc() // FIX: kind loc + target loc
  let kind_str = match kind {
    Value => "value"
    Type => "type"
    Trait => "trait"
  }
  tagged_node("UsingName", loc, {
    "name": target.json_repr(),
    "kind": Json::string(kind_str),
  })
}

///|
pub fn Argument::json_repr(self : Argument) -> Json {
  tagged_node("Argument", self.value.loc(), {
    "value": self.value.json_repr(),
    "kind": self.kind.json_repr(self.value.loc()),
  })
}

///|
pub fn ArgumentKind::json_repr(self : ArgumentKind, loc : Location) -> Json {
  match self {
    Positional => tagged_node0("ArgumentKind::Positional", loc)
    Labelled(label) =>
      tagged_node1("ArgumentKind::Labelled", label.loc, label.json_repr())
    LabelledPun(label) =>
      tagged_node1("ArgumentKind::LabelledPun", label.loc, label.json_repr())
    LabelledOption(label~, question_loc~) => {
      ignore(question_loc)
      tagged_node("ArgumentKind::LabelledOption", label.loc, {
        "label": label.json_repr(),
      })
    }
    LabelledOptionPun(label~, question_loc~) => {
      ignore(question_loc)
      tagged_node("ArgumentKind::LabelledOptionPun", label.loc, {
        "label": label.json_repr(),
      })
    }
  }
}

///|
pub fn ApplyAttr::json_repr(self : ApplyAttr, loc : Location) -> Json {
  match self {
    NoAttr => tagged_node0("ApplyAttr::NoAttr", loc)
    Exclamation => tagged_node0("ApplyAttr::Exclamation", loc)
    Question => tagged_node0("ApplyAttr::Question", loc)
  }
}

///|
pub fn Var::json_repr(self : Var) -> Json {
  tagged_node("Var", self.loc, { "name": self.name.json_repr(self.loc) })
}

///|
pub fn Constant::json_repr(self : Constant, loc : Location) -> Json {
  match self {
    Bool(b) => tagged_node1("Constant::Bool", loc, Json::boolean(b))
    Byte(b) => tagged_node1("Constant::Byte", loc, byte_literal_json_repr(b))
    Bytes(b) => tagged_node1("Constant::Bytes", loc, bytes_literal_json_repr(b))
    Char(c) => tagged_node1("Constant::Char", loc, char_literal_json_repr(c))
    Int(s) => tagged_node1("Constant::Int", loc, Json::string(s))
    Int64(s) => tagged_node1("Constant::Int64", loc, Json::string(s))
    UInt(s) => tagged_node1("Constant::UInt", loc, Json::string(s))
    UInt64(s) => tagged_node1("Constant::UInt64", loc, Json::string(s))
    Float(s) => tagged_node1("Constant::Float", loc, Json::string(s))
    Double(s) => tagged_node1("Constant::Double", loc, Json::string(s))
    String(s) =>
      tagged_node1("Constant::String", loc, string_literal_json_repr(s))
    BigInt(s) => tagged_node1("Constant::BigInt", loc, Json::string(s))
  }
}

///|
pub fn SpreadableElem::json_repr(self : SpreadableElem) -> Json {
  match self {
    Regular(expr) =>
      tagged_node1("SpreadableElem::Regular", expr.loc(), expr.json_repr())
    Spread(expr~, loc~) =>
      tagged_node("SpreadableElem::Spread", loc, { "expr": expr.json_repr() })
  }
}

///|
pub fn MultilineStringElem::json_repr(
  self : MultilineStringElem,
  loc : Location,
) -> Json {
  match self {
    String(s) => tagged_node1("MultilineString::String", loc, Json::string(s))
    Interp(elems) =>
      tagged_node1(
        "MultilineString::Interp",
        loc,
        list_json_repr(
          "MultilineString::Interp::ElemList",
          loc,
          elems,
          InterpElem::json_repr,
        ),
      )
  }
}

///|
pub fn InterpElem::json_repr(self : InterpElem) -> Json {
  match self {
    Literal(repr~, loc~) =>
      tagged_node("InterpElem::Literal", loc, {
        "repr": string_literal_json_repr(repr),
      })
    Expr(expr~, loc~) =>
      tagged_node("InterpElem::Expr", loc, { "expr": expr.json_repr() })
    Source({ source, loc }) =>
      tagged_node("InterpElem::Source", loc, { "source": source })
  }
}

///|
pub fn Constructor::json_repr(self : Constructor) -> Json {
  tagged_node("Constructor", self.loc, {
    "name": self.name.json_repr(),
    "extra_info": self.extra_info.json_repr(self.loc),
  })
}

///|
pub fn ConstructorExtraInfo::json_repr(
  self : ConstructorExtraInfo,
  loc : Location,
) -> Json {
  match self {
    TypeName(type_name) =>
      tagged_node1(
        "ConstructorExtraInfo::TypeName",
        type_name.loc,
        type_name.json_repr(),
      )
    Package(s) =>
      tagged_node1("ConstructorExtraInfo::Package", loc, Json::string(s))
    NoExtraInfo => tagged_node0("ConstructorExtraInfo::NoExtraInfo", loc)
  }
}

///|
pub fn Func::json_repr(self : Func) -> Json {
  tagged_node("Func::Lambda", self.loc, {
    "parameters": list_json_repr(
      "Func::Lambda::ParameterList",
      self.params_loc,
      self.parameters,
      Parameter::json_repr,
    ),
    "body": self.body.json_repr(),
    "return_type": option_json_repr(self.return_type, Type::json_repr),
    "error_type": self.error_type.json_repr(self.loc),
    "kind": self.kind.json_repr(self.loc),
    "has_error": option_json_repr(self.has_error, raise_json_repr),
    "is_async": option_json_repr(self.is_async, async_json_repr),
  })
}

///|
pub fn FnKind::json_repr(self : FnKind, loc : Location) -> Json {
  match self {
    Lambda => tagged_node0("FnKind::Lambda", loc)
    Arrow => tagged_node0("FnKind::Arrow", loc)
  }
}

///|
pub fn FieldDef::json_repr(self : FieldDef) -> Json {
  tagged_node("FieldDef", self.loc, {
    "label": self.label.json_repr(),
    "expr": self.expr.json_repr(),
    "is_pun": Json::boolean(self.is_pun),
  })
}

///|
pub fn TrailingMark::json_repr(self : TrailingMark, loc : Location) -> Json {
  match self {
    Comma => tagged_node0("Trailing::Comma", loc)
    Semi => tagged_node0("Trailing::Semi", loc)
    None => tagged_node0("Trailing::None", loc)
  }
}

///|
pub fn Accessor::json_repr(self : Accessor) -> Json {
  match self {
    Label(label) =>
      tagged_node1("Accessor::Label", label.loc, label.json_repr())
    Index(tuple_index~, loc~) =>
      tagged_node("Accessor::Index", loc, {
        "tuple_index": Json::number(tuple_index.to_double()),
      })
    Newtype(loc~) => tagged_node0("Accessor::Newtype", loc)
  }
}

///|
pub fn Case::json_repr(self : Case) -> Json {
  let pattern_loc = self.pattern.loc()
  let body_loc = self.body.loc()
  let loc = pattern_loc.merge(body_loc)
  tagged_node("Case", loc, {
    "pattern": self.pattern.json_repr(),
    "guard": option_json_repr(self.guard_, Expr::json_repr),
    "body": self.body.json_repr(),
  })
}

///|
pub fn Pattern::json_repr(self : Pattern) -> Json {
  match self {
    Alias(pat~, alias_~, loc~) =>
      tagged_node("Pattern::Alias", loc, {
        "pat": pat.json_repr(),
        "alias": alias_.json_repr(),
      })
    Any(loc~) => tagged_node0("Pattern::Any", loc)
    Array(pats~, loc~) =>
      tagged_node("Pattern::Array", loc, { "pats": pats.json_repr(loc) })
    Constant(c~, loc~) =>
      tagged_node("Pattern::Constant", loc, { "constant": c.json_repr(loc) })
    Constraint(pat~, ty~, loc~) =>
      tagged_node("Pattern::Constraint", loc, {
        "pat": pat.json_repr(),
        "ty": ty.json_repr(),
      })
    Constr(constr~, args~, is_open~, loc~) =>
      tagged_node("Pattern::Constr", loc, {
        "constr": constr.json_repr(),
        "args": option_json_repr(args, fn(args) {
          list_json_repr(
            "Pattern::Constr::ArgList",
            loc,
            args,
            ConstrPatArg::json_repr,
          )
        }),
        "is_open": Json::boolean(is_open),
      })
    Or(pat1~, pat2~, loc~) =>
      tagged_node("Pattern::Or", loc, {
        "pat1": pat1.json_repr(),
        "pat2": pat2.json_repr(),
      })
    Tuple(pats~, loc~) =>
      tagged_node("Pattern::Tuple", loc, {
        "pats": list_json_repr(
          "Pattern::Tuple::PatternList",
          loc,
          pats,
          Pattern::json_repr,
        ),
      })
    Var(binder) => tagged_node1("Pattern::Var", binder.loc, binder.json_repr())
    Record(fields~, is_closed~, loc~) =>
      tagged_node("Pattern::Record", loc, {
        "fields": list_json_repr(
          "Pattern::Record::FieldList",
          loc,
          fields,
          FieldPat::json_repr,
        ),
        "is_closed": Json::boolean(is_closed),
      })
    Map(elems~, is_closed~, loc~) =>
      tagged_node("Pattern::Map", loc, {
        "elems": list_json_repr(
          "Pattern::Map::ElemList",
          loc,
          elems,
          MapPatElem::json_repr,
        ),
        "is_closed": Json::boolean(is_closed),
      })
    Range(lhs~, rhs~, kind~, loc~) =>
      tagged_node("Pattern::Range", loc, {
        "lhs": lhs.json_repr(),
        "rhs": rhs.json_repr(),
        "kind": kind.json_repr(loc),
      })
    SpecialConstr(binder~, args~, loc~) =>
      tagged_node("Pattern::SpecialConstr", loc, {
        "binder": binder.json_repr(),
        "args": list_json_repr(
          "Pattern::SpecialConstr::ArgList",
          loc,
          args,
          ConstrPatArg::json_repr,
        ),
      })
  }
}

///|
pub fn RangeKind::json_repr(self : RangeKind, loc : Location) -> Json {
  match self {
    Inclusive | InclusiveMissingEqual =>
      tagged_node0("RangeKind::Inclusive", loc)
    Exclusive => tagged_node0("RangeKind::Exclusive", loc)
  }
}

///|
pub fn ArrayPatterns::json_repr(self : ArrayPatterns, loc : Location) -> Json {
  match self {
    Closed(pats) =>
      tagged_node1(
        "ArrayPatterns::Closed",
        loc,
        list_json_repr(
          "ArrayPatterns::Closed::PatternList",
          loc,
          pats,
          ArrayPattern::json_repr,
        ),
      )
    Open(pats1, pats2, binder) =>
      tagged_node("ArrayPatterns::Open", loc, {
        "0": list_json_repr(
          "ArrayPatterns::Open::LPatternList",
          loc,
          pats1,
          ArrayPattern::json_repr,
        ),
        "1": list_json_repr(
          "ArrayPatterns::Open::RPatternList",
          loc,
          pats2,
          ArrayPattern::json_repr,
        ),
        "2": binder.json_repr(loc),
      })
  }
}

///|
pub fn ArrayPattern::json_repr(self : ArrayPattern) -> Json {
  match self {
    Pattern(pattern) =>
      tagged_node1("ArrayPattern::Pattern", pattern.loc(), pattern.json_repr())
    StringSpread(str~, loc~) =>
      tagged_node("ArrayPattern::StringSpread", loc, {
        "str": string_literal_json_repr(str),
      })
    BytesSpread(bytes~, loc~) =>
      tagged_node("ArrayPattern::BytesSpread", loc, {
        "bytes": bytes_literal_json_repr(bytes),
      })
    ConstSpread(binder~, pkg~, loc~) =>
      tagged_node("ArrayPattern::ConstSpread", loc, {
        "binder": binder.json_repr(),
        "pkg": option_json_repr(pkg, Json::string),
      })
  }
}

///|
pub fn DotDotBinder::json_repr(self : DotDotBinder, loc : Location) -> Json {
  match self {
    Underscore => tagged_node0("DotDotBinder::Underscore", loc)
    NoBinder => tagged_node0("DotDotBinder::NoBinder", loc)
    BinderAs(binder) =>
      tagged_node1("DotDotBinder::BinderAs", binder.loc, binder.json_repr())
    Binder(binder) =>
      tagged_node1("DotDotBinder::Binder", binder.loc, binder.json_repr())
  }
}

///|
pub fn ConstrPatArg::json_repr(self : ConstrPatArg) -> Json {
  tagged_node("ConstrPatArg", self.pat.loc(), {
    "pat": self.pat.json_repr(),
    "kind": self.kind.json_repr(self.pat.loc()),
  })
}

///|
pub fn FieldPat::json_repr(self : FieldPat) -> Json {
  tagged_node("FieldPat", self.loc, {
    "label": self.label.json_repr(),
    "pattern": self.pattern.json_repr(),
    "is_pun": Json::boolean(self.is_pun),
  })
}

///|
pub fn MapPatElem::json_repr(self : MapPatElem) -> Json {
  tagged_node("MapPatElem", self.loc, {
    "key": self.key.json_repr(self.key_loc),
    "pat": self.pat.json_repr(),
    "match_absent": Json::boolean(self.match_absent),
  })
}

///|
pub fn MapExprElem::json_repr(self : MapExprElem) -> Json {
  tagged_node("MapExprElem", self.loc, {
    "key": self.key.json_repr(self.key_loc),
    "expr": self.expr.json_repr(),
  })
}

///|
pub fn StaticAssertion::json_repr(self : StaticAssertion) -> Json {
  tagged_node("StaticAssertion", self.loc, {
    "ty": self.ty.json_repr(),
    "trait": self.trait_.json_repr(self.loc),
    "msg": Json::string(self.msg),
  })
}

///|
pub fn LexCase::json_repr(self : LexCase, loc : Location) -> Json {
  tagged_node("LexCase", loc, {
    "pat": list_json_repr("LexCase::PatternList", self.pat_loc, self.pat, fn(
      ltp,
    ) {
      ltp.json_repr(self.pat_loc)
    }),
    "body": self.body.json_repr(),
  })
}

///|
pub fn LexTopPattern::json_repr(self : LexTopPattern, loc : Location) -> Json {
  match self {
    Pattern(pat) => tagged_node1("LexTopPattern::Pattern", loc, pat.json_repr())
    Binder(binder) =>
      tagged_node1("LexTopPattern::Binder", loc, binder.json_repr())
    Wildcard(loc~) => tagged_node0("LexTopPattern::Wildcard", loc)
  }
}

///|
pub fn LexPattern::json_repr(self : LexPattern) -> Json {
  match self {
    Regex(lit~, loc~) =>
      tagged_node("LexPattern::Regex", loc, { "lit": Json::string(lit) })
    RegexInterp(elems~, loc~) =>
      tagged_node("LexPattern::RegexInterp", loc, {
        "elems": list_json_repr(
          "LexPattern::RegexInterp::InterpElemList",
          loc,
          elems,
          InterpElem::json_repr,
        ),
      })
    Alias(pat~, binder~, loc~) =>
      tagged_node("LexPattern::Alias", loc, {
        "pat": pat.json_repr(),
        "binder": binder.json_repr(),
      })
    Sequence(pats~, loc~) =>
      tagged_node("LexPattern::Sequence", loc, {
        "pats": list_json_repr(
          "LexPattern::Sequence::PatternList",
          loc,
          pats,
          LexPattern::json_repr,
        ),
      })
  }
}

///|
pub fn TryOperatorKind::json_repr(
  self : TryOperatorKind,
  loc : Location,
) -> Json {
  match self {
    Question => tagged_node0("TryOperatorKind::Question", loc)
    Exclamation => tagged_node0("TryOperatorKind::Exclamation", loc)
  }
}

///|
pub fn Group::json_repr(self : Group, loc : Location) -> Json {
  match self {
    Brace => tagged_node0("Group::Brace", loc)
    Paren => tagged_node0("Group::Paren", loc)
  }
}

///|
pub fn Hole::json_repr(self : Hole, loc : Location) -> Json {
  match self {
    Synthesized => tagged_node0("Hole::Synthesized", loc)
    Incomplete => tagged_node0("Hole::Incomplete", loc)
    Todo => tagged_node0("Hole::Todo", loc)
  }
}

///|
fn async_json_repr(is_async : Location) -> Json {
  tagged_node0("Async", is_async)
}

///|
fn raise_json_repr(loc : Location) -> Json {
  tagged_node0("Raise", loc)
}

///|
pub fn Impl::json_repr(self : Impl) -> Json {
  match self {
    TopExpr(expr~, is_main~, local_types~, is_async~, loc~) =>
      tagged_node("Impl::TopExpr", loc, {
        "expr": expr.json_repr(),
        "is_main": Json::boolean(is_main),
        "local_types": list_json_repr(
          "Impl::TopExpr::LocalTypeList",
          loc,
          local_types,
          LocalTypeDecl::json_repr,
        ),
        "is_async": option_json_repr(is_async, async_json_repr),
      })
    TopTest(expr~, name~, params~, local_types~, is_async~, loc~, attrs~, doc~) =>
      tagged_node("Impl::TopTest", loc, {
        "expr": expr.json_repr(),
        "name": name.json_repr(),
        "params": option_json_repr(params, params => {
          list_json_repr(
            "Impl::TopTest::ParamList",
            loc,
            params,
            Parameter::json_repr,
          )
        }),
        "local_types": list_json_repr(
          "Impl::TopTest::LocalTypeList",
          loc,
          local_types,
          LocalTypeDecl::json_repr,
        ),
        "is_async": option_json_repr(is_async, async_json_repr),
        "attrs": list_json_repr(
          "Impl::TopTest::AttrList",
          loc,
          attrs,
          Attribute::json_repr,
        ),
        "doc": doc,
      })
    TopTypeDef(type_decl) =>
      tagged_node1("Impl::TopTypeDef", type_decl.loc, type_decl.json_repr())
    TopFuncDef(fun_decl~, decl_body~, loc~) =>
      tagged_node("Impl::TopFuncDef", loc, {
        "fun_decl": fun_decl.json_repr(loc),
        "decl_body": decl_body.json_repr(loc),
      })
    TopFuncAlias(pkg~, type_name~, targets~, vis~, attrs~, is_list~, doc~, loc~) =>
      tagged_node("Impl::TopFuncAlias", loc, {
        "pkg": option_json_repr(pkg, Label::json_repr),
        "type_name": option_json_repr(type_name, Label::json_repr),
        "targets": list_json_repr(
          "Impl::TopFuncAlias::TargetList",
          loc,
          targets,
          AliasTarget::json_repr,
        ),
        "vis": vis.json_repr(loc),
        "attrs": list_json_repr(
          "Impl::TopFuncAlias::AttrList",
          loc,
          attrs,
          Attribute::json_repr,
        ),
        "is_list": is_list,
        "doc": doc,
      })
    TopLetDef(
      binder~,
      ty~,
      expr~,
      vis~,
      is_constant~,
      loc~,
      attrs~,
      doc~,
      is_declare~
    ) =>
      tagged_node("Impl::TopLetDef", loc, {
        "binder": binder.json_repr(),
        "ty": option_json_repr(ty, Type::json_repr),
        "expr": expr.json_repr(),
        "vis": vis.json_repr(loc),
        "is_constant": is_constant,
        "is_declare": is_declare,
        "attrs": list_json_repr(
          "Impl::TopLetDef::AttrList",
          loc,
          attrs,
          Attribute::json_repr,
        ),
        "doc": doc,
      })
    TopTrait(trait_decl) =>
      tagged_node1("Impl::TopTrait", trait_decl.loc, trait_decl.json_repr())
    TopBatchTypeAlias(pkg~, targets~, vis~, loc~, attrs~, is_list~, doc~) =>
      tagged_node("Impl::TopBatchTypeAlias", loc, {
        "pkg": option_json_repr(pkg, Label::json_repr),
        "targets": list_json_repr(
          "Impl::TopBatchTypeAlias::TargetList",
          loc,
          targets,
          AliasTarget::json_repr,
        ),
        "vis": vis.json_repr(loc),
        "attrs": list_json_repr(
          "Impl::TopBatchTypeAlias::AttrList",
          loc,
          attrs,
          Attribute::json_repr,
        ),
        "is_list": is_list,
        "doc": doc,
      })
    TopBatchTraitAlias(pkg~, targets~, vis~, loc~, attrs~, is_list~, doc~) =>
      tagged_node("Impl::TopBatchTraitAlias", loc, {
        "pkg": option_json_repr(pkg, Label::json_repr),
        "targets": list_json_repr(
          "Impl::TopBatchTraitAlias::TargetList",
          loc,
          targets,
          AliasTarget::json_repr,
        ),
        "vis": vis.json_repr(loc),
        "attrs": list_json_repr(
          "Impl::TopBatchTraitAlias::AttrList",
          loc,
          attrs,
          Attribute::json_repr,
        ),
        "is_list": is_list,
        "doc": doc,
      })
    TopImpl(
      self_ty~,
      trait_~,
      method_name~,
      has_error~,
      quantifiers~,
      params~,
      ret_ty~,
      err_ty~,
      body~,
      vis~,
      loc~,
      attrs~,
      doc~
    ) =>
      tagged_node("Impl::TopImpl", loc, {
        "self_ty": option_json_repr(self_ty, Type::json_repr),
        "trait": trait_.json_repr(),
        "method_name": method_name.json_repr(),
        "has_error": option_json_repr(has_error, Location::to_json),
        "quantifiers": list_json_repr(
          "Impl::TopImpl::QuantifierList",
          loc,
          quantifiers,
          TypeVarBinder::json_repr,
        ),
        "params": list_json_repr(
          "Impl::TopImpl::ParamList",
          loc,
          params,
          Parameter::json_repr,
        ),
        "ret_ty": option_json_repr(ret_ty, Type::json_repr),
        "err_ty": err_ty.json_repr(loc),
        "body": body.json_repr(loc),
        "vis": vis.json_repr(loc),
        "attrs": list_json_repr(
          "Impl::TopImpl::AttrList",
          loc,
          attrs,
          Attribute::json_repr,
        ),
        "doc": doc,
      })
    TopView(
      quantifiers~,
      source_ty~,
      view_constrs~,
      body~,
      vis~,
      loc~,
      attrs~,
      view_type_name~,
      parameters~,
      view_func_name~,
      view_type_loc~,
      params_loc~,
      doc~
    ) => {
      ignore(view_type_loc)
      tagged_node("Impl::TopView", loc, {
        "quantifiers": list_json_repr(
          "Impl::TopView::QuantifierList",
          loc,
          quantifiers,
          TypeVarBinder::json_repr,
        ),
        "source_ty": source_ty.json_repr(),
        "view_constrs": list_json_repr(
          "Impl::TopView::ConstrList",
          params_loc,
          view_constrs,
          ConstrDecl::json_repr,
        ),
        "body": body.json_repr(),
        "ty_name": view_type_name,
        "parameters": list_json_repr(
          "Impl::TopView::ParameterList",
          loc,
          parameters,
          Parameter::json_repr,
        ),
        "func_name": view_func_name.json_repr(),
        "vis": vis.json_repr(loc),
        "attrs": list_json_repr(
          "Impl::TopView::AttrList",
          loc,
          attrs,
          Attribute::json_repr,
        ),
        "doc": doc,
      })
    }
    TopImplRelation(
      self_ty~,
      trait_~,
      quantifiers~,
      vis~,
      attrs~,
      doc~,
      loc~,
      is_declare~
    ) =>
      tagged_node("Impl::TopImplRelation", loc, {
        "self_ty": self_ty.json_repr(),
        "trait": trait_.json_repr(),
        "quantifiers": list_json_repr(
          "Impl::TopImplRelation::QuantifierList",
          loc,
          quantifiers,
          TypeVarBinder::json_repr,
        ),
        "vis": vis.json_repr(loc),
        "is_declare": is_declare,
        "attrs": list_json_repr(
          "Impl::TopImplRelation::AttrList",
          loc,
          attrs,
          Attribute::json_repr,
        ),
        "doc": doc,
      })
    TopUsing(pkg~, names~, vis~, attrs~, loc~, doc~) =>
      tagged_node("Impl::TopUsing", loc, {
        "pkg": pkg.json_repr(),
        "names": list_json_repr(
          "Impl::TopUsing::NameList", loc, names, using_name_json_repr,
        ),
        "vis": vis.json_repr(loc),
        "attrs": list_json_repr(
          "Impl::TopUsing::AttrList",
          loc,
          attrs,
          Attribute::json_repr,
        ),
        "doc": doc,
      })
  }
}
