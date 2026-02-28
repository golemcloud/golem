///|
priv enum ExprOrBinderFragment {
  Binder(@syntax.Expr, colon_loc~ : Location, ty~ : @syntax.Type)
  Expr(@syntax.Expr)
}

///|
fn State::peek_spos_with_parsed(
  self : Self,
  parsed~ : @syntax.Expr?,
) -> Position {
  match parsed {
    None => self.peek_spos()
    Some(e) => e.loc().start
  }
}

///|
fn State::parse_optional_bang(self : Self) -> Location? {
  match self.peek_token() {
    EXCLAMATION => {
      let loc = self.peek_location()
      self.skip()
      Some(loc)
    }
    _ => None
  }
}

///|
fn State::parse_attributes(self : Self) -> List[Attribute] {
  let result = []
  while self.peek_token() is ATTRIBUTE(content) {
    let loc = self.peek_location()
    result.push(Attribute::new(loc~, content))
    self.skip()
  }
  @list.from_array(result)
}

///|
fn State::parse_toplevel(self : Self) -> Impls {
  guard self.mode is Normal else { @list.new() }
  let result = []
  self.push_sync(TK_EOF)
  while self.mode is Normal && !(self.peek_token() is EOF) {
    self.push_syncs(first_impl_)
    result.push(self.parse_top())
    if self.mode is Normal {
      match self.peek_token() {
        SEMI(_) => self.skip()
        EOF => ()
        other => {
          self.panic() |> ignore
          let loc = self.peek_location()
          let report : Report = if first_impl_.contains(other.kind()) {
            { loc, msg: "Missing newline here." }
          } else {
            { loc, msg: "Unexpected token here." }
          }
          self.report_error(report)
        }
      }
    }
    self.pop_syncs(first_impl_)
  }
  self.pop_sync(TK_EOF)
  @list.from_array(result)
}

///|
fn State::parse_qual_lident(
  self : Self,
  context~ : String,
) -> @syntax.LongIdent {
  if self.mode is Panic(_) {
    return @syntax.LongIdent::dummy()
  }
  match self.peek_token() {
    LIDENT(id) => {
      self.skip()
      Ident(name=id)
    }
    UIDENT(_) => {
      // error recovery
      self.skip()
      @syntax.LongIdent::dummy()
    }
    PACKAGE_NAME(pkg) => {
      self.skip()
      let id = self.expect_dot_lident(context~)
      Dot(pkg~, id~)
    }
    other => {
      let loc = self.peek_location()
      self.report_unexpected(other, first_qual_lident, loc)
      self.panic() |> ignore
      @syntax.LongIdent::dummy()
    }
  }
}

///|
fn State::parse_qual_luident(self : Self) -> @syntax.LongIdent {
  guard self.mode is Normal else { @syntax.LongIdent::dummy() }
  match self.peek_token() {
    LIDENT(id) | UIDENT(id) => {
      self.skip()
      Ident(name=id)
    }
    PACKAGE_NAME(pkg) => {
      self.skip()
      match self.peek_token() {
        DOT_LIDENT(id) | DOT_UIDENT(id) => {
          self.skip()
          Dot(pkg~, id~)
        }
        other => {
          let loc = self.peek_location()
          self.report_unexpected(other, [TK_DOT_LIDENT, TK_DOT_UIDENT], loc)
          self.panic() |> ignore
          @syntax.LongIdent::dummy()
        }
      }
    }
    other => {
      let loc = self.peek_location()
      self.report_unexpected(other, first_qual_luident, loc)
      self.panic() |> ignore
      @syntax.LongIdent::dummy()
    }
  }
}

///|
fn State::parse_luident(self : Self) -> String {
  guard self.mode is Normal else { "" }
  match self.peek_token() {
    LIDENT(id) | UIDENT(id) => {
      self.skip()
      id
    }
    other => {
      let loc = self.peek_location()
      self.report_unexpected(other, first_luident, loc)
      self.panic() |> ignore
      ""
    }
  }
}

///|
fn State::parse_binder(self : Self, context~ : String) -> @syntax.Binder {
  if self.mode is Panic(loc~, ..) {
    return { name: "", loc }
  }
  let loc = self.peek_location()
  let name = self.expect_lident(context~)
  { name, loc }
}

///|
fn State::parse_type_name(self : Self) -> @syntax.TypeName {
  if self.mode is Panic(loc~, ..) {
    return { name: @syntax.LongIdent::dummy(), is_object: false, loc }
  }
  let spos = self.peek_spos()
  let is_object = match self.peek_token() {
    AMPER => {
      self.skip()
      true
    }
    _ => false
  }
  let name = self.parse_qual_luident()
  let loc = self.loc_start_with(spos)
  { name, is_object, loc }
}

///|
fn State::parse_fun_binder(self : Self) -> (@syntax.TypeName?, @syntax.Binder) {
  if self.mode is Panic(loc~, ..) {
    return (None, { name: "", loc })
  }
  fn parse_qual_with_coloncolon() {
    let type_name = self.parse_type_name()
    self.expect_token(TK_COLONCOLON)
    let loc_func_name = self.peek_location()
    let func_name = self.expect_lident(context="name of `fn`")
    (Some(type_name), @syntax.Binder::{ name: func_name, loc: loc_func_name })
  }

  match self.peek_token() {
    PACKAGE_NAME(_) | UIDENT(_) | AMPER => parse_qual_with_coloncolon()
    LIDENT(_) if self.peek_token(nth=1) is COLONCOLON =>
      parse_qual_with_coloncolon()
    LIDENT(_) => {
      let id = self.parse_binder(context="name of `fn`")
      (None, id)
    }
    other => {
      let loc = self.panic()
      self.report_unexpected(other, first_qual_lident, loc)
      (None, { name: "", loc })
    }
  }
}

///|
fn State::parse_var(self : Self) -> @syntax.Var {
  if self.mode is Panic(loc~, ..) {
    return { name: @syntax.LongIdent::dummy(), loc }
  }
  let spos = self.peek_spos()
  let name = self.parse_qual_lident(context="")
  let loc = self.loc_start_with(spos)
  { name, loc }
}

///|
fn State::parse_tvar_binder(self : Self) -> @syntax.TypeVarBinder {
  if self.mode is Panic(loc~, ..) {
    return {
      name: "?tvar_name",
      name_loc: loc,
      constraints: @list.from_array([]),
    }
  }
  let spos = self.peek_spos()
  let name = self.parse_luident()
  let loc = self.loc_start_with(spos)
  let constraints : @list.List[@syntax.TypeVarConstraint]? = self.optional(
    [TK_COLON],
    fn(state) {
      state.expect_token(TK_COLON)
      state.sepby1(delim=TK_PLUS, fn(state) {
        let loc = state.peek_location()
        let trait_ = state.parse_qual_luident()
        { trait_, loc }
      })
      |> @list.from_array
    },
  )
  let constraints = match constraints {
    None => @list.new()
    Some(xs) => xs
  }
  { name, name_loc: loc, constraints }
}

///|
fn State::parse_type_decl_binder(self : Self) -> @syntax.TypeDeclBinder {
  if self.mode is Panic(loc~, ..) {
    return { name: None, loc }
  }
  let spos = self.peek_spos()
  let name = match self.peek_token() {
    UNDERSCORE => {
      self.skip()
      None
    }
    _ => Some(self.parse_luident())
  }
  let loc = self.loc_start_with(spos)
  { name, loc }
}

///|
fn State::parse_constr(self : Self) -> @syntax.Constructor {
  if self.mode is Panic(loc~, ..) {
    return { name: { name: "", loc }, extra_info: NoExtraInfo, loc }
  }
  let spos = self.peek_spos()
  match self.peek_token() {
    UIDENT(_) if !(self.peek_token(nth=1) is COLONCOLON) => {
      // Constr
      let loc = self.peek_location()
      let name = self.parse_luident()
      let loc_constr = self.loc_start_with(spos)
      { name: { name, loc }, extra_info: NoExtraInfo, loc: loc_constr }
    }
    PACKAGE_NAME(pkg) if self.peek_token(nth=1) is DOT_UIDENT(_) &&
      !(self.peek_token(nth=2) is COLONCOLON) => {
      // @pkg.Constr
      self.skip()
      let name : @syntax.ConstrName = {
        let spos = self.peek_spos()
        let name = self.expect_dot_uident(context="")
        { name, loc: self.loc_start_with(spos) }
      }
      let loc = self.loc_start_with(spos)
      { name, extra_info: Package(pkg), loc }
    }
    LIDENT(_) | UIDENT(_) | PACKAGE_NAME(_) | AMPER(_) => {
      // @pkg.TypeName::Constr
      // TypeName::Constr
      // The "&" case is just for error recovery and consistency with yacc parser
      let type_name = self.parse_type_name()
      self.expect_token(TK_COLONCOLON)
      let right_loc = self.peek_location()
      let right = self.expect_uident(context="constructor name")
      let loc = self.loc_start_with(spos)
      {
        name: { name: right, loc: right_loc },
        extra_info: TypeName(type_name),
        loc,
      }
    }
    other => {
      let loc = self.peek_location()
      self.report_unexpected(other, first_constr, loc)
      self.panic() |> ignore
      { name: { name: "", loc }, extra_info: NoExtraInfo, loc }
    }
  }
}

///|
fn State::parse_opt_type_params_no_constraints(
  self : Self,
) -> List[@syntax.TypeDeclBinder] {
  if self.mode is Panic(_) {
    return @list.new()
  }
  self
  .optional([TK_LBRACKET], fn(state) {
    state.surround_series(
      left=TK_LBRACKET,
      right=TK_RBRACKET,
      delim=TK_COMMA,
      State::parse_type_decl_binder,
    )
  })
  .unwrap_or([])
  |> @list.from_array
}

///|
fn State::parse_local_type_decl(self : Self) -> @syntax.LocalTypeDecl {
  if self.mode is Panic(loc~, ..) {
    return {
      tycon: "?local_tycon",
      tycon_loc: loc,
      components: Variant(@list.new()),
      deriving: @list.new(),
    }
  }
  match self.peek_token() {
    STRUCT => {
      self.expect_token(TK_STRUCT)
      let tycon_loc = self.peek_location()
      let tycon = self.parse_luident()
      let components : @syntax.TypeDesc = match self.peek_token() {
        LPAREN => {
          self.skip() // consume '('
          let (ts, _) = self.series1_with_follow(
            follow_set=[TK_RPAREN],
            delim=TK_COMMA,
            State::parse_type,
          )
          self.expect_token(TK_RPAREN)
          TupleStruct(@list.from_array(ts))
        }
        _ => {
          let fields = self.surround_series(
            invalid_delims=[TK_COMMA],
            left=TK_LBRACE,
            right=TK_RBRACE,
            delim=TK_SEMI,
            State::parse_field_decl,
          )
          Record(@list.from_array(fields))
        }
      }
      let deriving = self.parse_deriving_directive_list()
      { tycon, tycon_loc, components, deriving }
    }
    ENUM => {
      self.expect_token(TK_ENUM)
      let tycon_loc = self.peek_location()
      let tycon = self.parse_luident()
      let constrs = self.surround_series(
          invalid_delims=[TK_COMMA],
          left=TK_LBRACE,
          right=TK_RBRACE,
          delim=TK_SEMI,
          State::parse_constr_decl,
        )
        |> @list.from_array
      let deriving = self.parse_deriving_directive_list()
      { tycon, tycon_loc, components: Variant(constrs), deriving }
    }
    TYPE => {
      self.expect_token(TK_TYPE)
      let tycon_loc = self.peek_location()
      let tycon = self.parse_luident()
      let typ = self.parse_type()
      let deriving = self.parse_deriving_directive_list()
      { tycon, tycon_loc, components: Newtype(typ), deriving }
    }
    other => {
      let other_loc = self.peek_location()
      self.report_unexpected(other, [TK_STRUCT, TK_ENUM, TK_TYPE], other_loc)
      let loc = self.panic()
      {
        tycon: "?local_tycon",
        tycon_loc: loc,
        components: Variant(@list.new()),
        deriving: @list.new(),
      }
    }
  }
}

///|
fn State::parse_fun_decl(
  self : Self,
  attrs~ : @list.List[@attribute.Attribute],
) -> @syntax.FunDecl {
  if self.mode is Panic(loc~, ..) {
    return {
      name: @syntax.Binder::{ name: "", loc },
      has_error: None,
      is_async: None,
      type_name: None,
      decl_params: Some(@list.new()),
      params_loc: loc,
      quantifiers: @list.new(),
      return_type: None,
      error_type: NoErrorType,
      vis: Default,
      doc: @syntax.DocString::empty(),
      attrs,
    }
  }
  let is_async = if self.peek_token() is ASYNC {
    let loc = self.peek_location()
    self.skip()
    Some(loc)
  } else {
    None
  }
  self.expect_token(TK_FN)
  let quantifiers = self.parse_opt_typ_params()
  let (type_name, name) = self.parse_fun_binder()
  let has_error = self.parse_optional_bang()
  let quantifiers = match quantifiers {
    Empty => self.parse_opt_typ_params()
    More(_, ..) => quantifiers
  }
  let param_loc_start = self.peek_spos()
  let decl_params = self.optional([TK_LPAREN], fn(state) {
    state.surround_series(
      left=TK_LPAREN,
      right=TK_RPAREN,
      delim=TK_COMMA,
      State::parse_parameter,
    )
    |> @list.from_array
  })
  let params_loc = self.loc_start_with(param_loc_start)
  let (return_type, error_type) = self.parse_func_return_type()
  {
    name,
    has_error,
    is_async,
    type_name,
    quantifiers,
    params_loc,
    decl_params,
    return_type,
    error_type,
    vis: Default,
    doc: @syntax.DocString::empty(),
    attrs,
  }
}

///|
fn State::parse_trait_method_decl(self : Self) -> @syntax.TraitMethodDecl {
  if self.mode is Panic(loc~, ..) {
    return {
      name: { name: "", loc },
      has_error: None,
      is_async: None,
      quantifiers: @list.new(),
      params: @list.new(),
      return_type: None,
      error_type: NoErrorType,
      has_default: None,
      attrs: @list.empty(),
      loc,
    }
  }
  let spos = self.peek_spos()
  let attrs = self.parse_attributes()
  self.push_sync(TK_LPAREN)
  let is_async = if self.peek_token() is ASYNC {
    let loc = self.peek_location()
    self.skip()
    Some(loc)
  } else {
    None
  }
  let func_name = self.parse_binder(context="trait method name")
  let has_error = self.parse_optional_bang()
  let quantifiers = self.parse_opt_typ_params()
  self.pop_sync(TK_LPAREN)
  self.expect_token(TK_LPAREN)
  let (params, _) = self.series_with_follow(
    delim=TK_COMMA,
    follow_set=[TK_RPAREN],
    fn(state) -> @syntax.Parameter {
      match state.peek_token() {
        // `label~ : Type`
        POST_LABEL(binder_name) => {
          let binder_loc = Location::trim_last_char(state.peek_location())
          state.skip()
          let binder : @syntax.Binder = { name: binder_name, loc: binder_loc }
          state.expect_token(TK_COLON)
          let typ = state.parse_type()
          Labelled(binder~, ty=Some(typ))
        }
        LIDENT(_) => {
          let binder_loc = state.peek_location()
          let binder_name = state.expect_lident(context="parameter name")
          let binder : @syntax.Binder = { name: binder_name, loc: binder_loc }
          let has_question = state.peek_token() is QUESTION
          if has_question {
            state.skip()
          }
          state.expect_token(TK_COLON)
          let typ = state.parse_type()
          if has_question {
            QuestionOptional(binder~, ty=Some(typ))
          } else {
            Positional(binder~, ty=Some(typ))
          }
        }
        tok if first_type_.contains(tok.kind()) => {
          let typ = state.parse_type()
          let loc = typ.loc()
          DiscardPositional(ty=Some(typ), loc~)
        }
        other => {
          let other_loc = state.peek_location()
          state.report_unexpected(
            other,
            [TK_POST_LABEL, ..first_type_],
            other_loc,
          )
          let loc = state.panic()
          DiscardPositional(ty=None, loc~)
        }
      }
    },
  )
  let params = params |> @list.from_array
  self.expect_token(TK_RPAREN)
  let (return_type, error_type) = self.parse_func_return_type()
  let has_default = if self.peek_token() is EQUAL {
    self.skip()
    let loc = self.peek_location()
    self.expect_token(TK_UNDERSCORE)
    Some(loc)
  } else {
    None
  }
  let loc = self.loc_start_with(spos)
  {
    name: func_name,
    has_error,
    is_async,
    quantifiers,
    params,
    return_type,
    error_type,
    has_default,
    attrs,
    loc,
  }
}

///|
fn State::parse_trait_decl(
  self : Self,
  vis~ : @syntax.Visibility,
  attrs~ : List[@attribute.Attribute],
  is_declare~ : Bool,
) -> @syntax.TraitDecl {
  if self.mode is Panic(loc~, ..) {
    return {
      name: { name: "", loc },
      supers: @list.new(),
      methods: @list.new(),
      vis: Default,
      loc,
      attrs,
      doc: @syntax.DocString::empty(),
      is_declare,
    }
  }
  let spos = self.peek_spos()
  self.expect_token(TK_TRAIT)
  let name_loc = self.peek_location()
  let name = self.parse_luident()
  let trait_name : @syntax.Binder = { name, loc: name_loc }
  let supers = if self.peek_token() is COLON {
    self.skip()
    self.with_follow(follow_set=[TK_LBRACE], fn(state) {
      state.sepby1(delim=TK_PLUS, fn(state) {
        let loc = state.peek_location()
        let trait_ = state.parse_qual_luident()
        @syntax.TypeVarConstraint::{ trait_, loc }
      })
      |> @list.from_array
    })
  } else {
    @list.new()
  }
  self.expect_token(TK_LBRACE)
  let (methods, _) = self.series_with_follow(
    delim=TK_SEMI,
    follow_set=[TK_RBRACE],
    State::parse_trait_method_decl,
  )
  let methods = @list.from_array(methods)
  self.expect_token(TK_RBRACE)
  let loc = self.loc_start_with(spos)
  {
    name: trait_name,
    supers,
    methods,
    vis,
    loc,
    attrs,
    doc: @syntax.DocString::empty(),
    is_declare,
  }
}

///|
fn State::parse_visibility(self : Self) -> @syntax.Visibility {
  if self.mode is Panic(..) {
    return Default
  }
  match self.peek_token() {
    PRIV => {
      let loc = self.peek_location()
      self.expect_token(TK_PRIV)
      Priv(loc~)
    }
    PUB => {
      let spos = self.peek_spos()
      self.expect_token(TK_PUB)
      let pub_attr = self.optional([TK_LPAREN], fn(state) {
        state.surround(left=TK_LPAREN, right=TK_RPAREN, fn(state) {
          match state.peek_token() {
            READONLY => {
              state.skip()
              "readonly"
            }
            LIDENT(attr) => {
              state.skip()
              attr
            }
            tok => {
              let loc = state.peek_location()
              state.panic() |> ignore
              state.report_unexpected(tok, [TK_READONLY, TK_LIDENT], loc)
              ""
            }
          }
        })
      })
      let loc = self.loc_start_with(spos)
      Pub(attr=pub_attr, loc~)
    }
    _ => Default
  }
}

///|
fn State::parse_typealias(
  self : Self,
  spos~ : Position,
  attrs~ : List[@attribute.Attribute],
  vis~ : @syntax.Visibility,
  is_declare~ : Bool,
) -> @syntax.Impl {
  self.expect_token(TK_TYPEALIAS)
  fn parse_target(
    self : State,
    target_name,
    target_loc,
  ) -> @syntax.AliasTarget {
    match self.peek_token() {
      AS => {
        self.skip()
        let binder_loc = self.peek_location()
        let binder_name = self.parse_luident()
        {
          target: Some({ name: target_name, loc: target_loc }),
          binder: { name: binder_name, loc: binder_loc },
        }
      }
      _ => { target: None, binder: { name: target_name, loc: target_loc } }
    }
  }

  match self.peek_token() {
    PACKAGE_NAME(pkg) if self.peek_token(nth=1) is DOT_LPAREN ||
      (
        self.peek_token(nth=1) is (DOT_LIDENT(_) | DOT_UIDENT(_)) &&
        self.peek_token(nth=2) is (EQUAL | LBRACKET)
      ) => {
      // typealias @pkg.Ty
      // typealias @pkg.Ty as Alias
      // typealias @pkg.(..)
      let pkg_loc = self.peek_location()
      let pkg : @syntax.Label? = Some({ name: pkg, loc: pkg_loc })
      self.skip()
      let (is_list, targets) = match self.peek_token() {
        DOT_LPAREN => {
          let targets = self.surround_series(
            left=TK_DOT_LPAREN,
            right=TK_LPAREN,
            delim=TK_COMMA,
            fn(state) {
              let target_loc = self.peek_location()
              let target_name = self.parse_luident()
              parse_target(state, target_name, target_loc)
            },
          )
          (true, targets)
        }
        DOT_LIDENT(target_name) | DOT_UIDENT(target_name) => {
          let target_loc = self.peek_location()
          self.skip()
          let target = parse_target(self, target_name, target_loc)
          (false, [target])
        }
        other => {
          let other_loc = self.peek_location()
          self.report_unexpected(
            other,
            [TK_DOT_LPAREN, TK_LIDENT, TK_UIDENT],
            other_loc,
          )
          self.panic() |> ignore
          (false, [])
        }
      }
      let loc = self.loc_start_with(spos)
      TopBatchTypeAlias(
        pkg~,
        targets=@list.from_array(targets),
        vis~,
        attrs~,
        loc~,
        is_list~,
        doc=DocString::empty(),
      )
    }
    LIDENT(target_name) | UIDENT(target_name) if !(self.peek_token(nth=1)
      is (EQUAL | LBRACKET)) => {
      // typealias Ty
      // typealias Ty as Alias
      let target_loc = self.peek_location()
      self.skip()
      let target = parse_target(self, target_name, target_loc)
      let loc = self.loc_start_with(spos)
      TopBatchTypeAlias(
        pkg=None,
        targets=@list.from_array([target]),
        vis~,
        attrs~,
        loc~,
        is_list=false,
        doc=DocString::empty(),
      )
    }
    token if first_type_.contains(token.kind()) => {
      let binder_or_target = self.parse_type()
      match self.peek_token() {
        // EQUAL => TopLegacyTypeAlias 
        AS => {
          self.skip()
          let tycon_loc = self.peek_location()
          let tycon = self.parse_luident()
          let params = self.parse_opt_type_params_no_constraints()
          let loc = self.loc_start_with(spos)
          TopTypeDef({
            type_vis: vis,
            tycon,
            tycon_loc,
            params,
            components: Alias(binder_or_target),
            deriving: @list.new(),
            attrs,
            doc: DocString::empty(),
            loc,
            is_declare,
          })
        }
        other => {
          let other_loc = self.peek_location()
          self.report_unexpected(other, [TK_AS], other_loc)
          let loc = self.panic()
          TopExpr(
            expr=Hole(loc~, kind=Synthesized),
            is_main=false,
            local_types=@list.new(),
            is_async=None,
            loc~,
          )
        }
      }
    }
    other => {
      let other_loc = self.peek_location()
      self.report_unexpected(
        other,
        [TK_UIDENT, TK_LIDENT, TK_LPAREN, TK_PACKAGE_NAME],
        other_loc,
      )
      let loc = self.panic()
      TopExpr(
        expr=Hole(loc~, kind=Synthesized),
        is_main=false,
        local_types=@list.new(),
        is_async=None,
        loc~,
      )
    }
  }
}

///|
fn State::parse_test_block(
  self : Self,
  attrs~ : List[Attribute],
  spos~ : Position,
  is_async~ : Location?,
) -> @syntax.Impl {
  self.expect_token(TK_TEST)
  let name : @syntax.TestName = if self.peek_token() is STRING(_) {
    let name_loc = self.peek_location()
    let v = self.expect_string()
    Some((v, name_loc))
  } else {
    None
  }
  let params = if self.peek_token() is LPAREN {
    let params = self.surround_series(
      left=TK_LPAREN,
      right=TK_RPAREN,
      delim=TK_COMMA,
      State::parse_parameter,
    )
    Some(@list.from_array(params))
  } else {
    None
  }
  let (local_types, expr) = self.parse_block_expr_with_local_types()
  let loc = self.loc_start_with(spos)
  TopTest(
    name~,
    params~,
    expr~,
    is_async~,
    local_types~,
    loc~,
    doc=DocString::empty(),
    attrs~,
  )
}

///|
fn State::parse_top_func(
  self : Self,
  attrs~ : List[Attribute],
  vis~ : @syntax.Visibility,
  spos~ : Position,
  is_declare~ : Bool,
) -> @syntax.Impl {
  let fun_decl = self.parse_fun_decl(attrs~)
  let fun_decl = { ..fun_decl, vis, }
  let decl_body : @syntax.DeclBody = if is_declare {
    DeclNone
  } else if self.peek_token() is EQUAL {
    self.skip()
    if self.peek_token() is MULTILINE_STRING(_) {
      let xs = []
      while self.peek_token() is MULTILINE_STRING(x) {
        xs.push(x)
        self.skip()
      }
      DeclStubs(
        Embedded(language=None, code=CodeMultilineString(@list.from_array(xs))),
      )
    } else {
      let name1 = self.expect_string()
      if self.peek_token() is STRING(name2) {
        self.skip()
        DeclStubs(Import(module_name=name1, func_name=name2, language=None))
      } else {
        DeclStubs(Embedded(language=None, code=CodeString(name1)))
      }
    }
  } else {
    let (local_types, body) = self.parse_block_expr_with_local_types()
    DeclBody(expr=body, local_types~)
  }
  let loc = self.loc_start_with(spos)
  TopFuncDef(fun_decl~, decl_body~, loc~)
}

///|
fn State::parse_top(self : Self) -> @syntax.Impl {
  if self.mode is Panic(loc~, ..) {
    return @syntax.TopExpr(
      expr=@syntax.Hole(loc~, kind=Synthesized),
      is_main=false,
      local_types=@list.new(),
      is_async=None,
      loc~,
    )
  }
  let spos = self.peek_spos()
  let attrs = self.parse_attributes()
  let is_declare = if self.peek_token() is DECLARE {
    self.skip()
    true
  } else {
    false
  }
  let vis = self.parse_visibility()
  match self.peek_token() {
    TYPE if self.peek_token(nth=1) is EXCLAMATION => {
      self.expect_token(TK_TYPE)
      self.expect_token(TK_EXCLAMATION)
      let tycon_loc = self.peek_location()
      let tycon = self.parse_luident()
      let components : @syntax.TypeDesc = match self.peek_token() {
        LBRACE => {
          let cs = self.surround_series(
              invalid_delims=[TK_COMMA],
              left=TK_LBRACE,
              right=TK_RBRACE,
              delim=TK_SEMI,
              State::parse_constr_decl,
            )
            |> @list.from_array
          Error(EnumPayload(cs))
        }
        _ => {
          let typ = self.optional(first_type_, State::parse_type)
          let exception_decl = match typ {
            Some(ty) => @syntax.SinglePayload(ty)
            None => NoPayload
          }
          Error(exception_decl)
        }
      }
      let deriving = self.parse_deriving_directive_list()
      let loc = self.loc_start_with(spos)
      @syntax.TopTypeDef({
        tycon,
        tycon_loc,
        params: @list.new(),
        components,
        type_vis: vis,
        doc: @syntax.DocString::empty(),
        deriving,
        loc,
        attrs,
        is_declare,
      })
    }
    SUBERROR => {
      self.expect_token(TK_SUBERROR)
      let tycon_loc = self.peek_location()
      let tycon = self.parse_luident()
      let components : @syntax.TypeDesc = match self.peek_token() {
        LBRACE => {
          let cs = self.surround_series(
              invalid_delims=[TK_COMMA],
              left=TK_LBRACE,
              right=TK_RBRACE,
              delim=TK_SEMI,
              State::parse_constr_decl,
            )
            |> @list.from_array
          Error(EnumPayload(cs))
        }
        _ => {
          let typ = self.optional(first_type_, State::parse_type)
          let exception_decl = match typ {
            Some(ty) => @syntax.SinglePayload(ty)
            None => NoPayload
          }
          Error(exception_decl)
        }
      }
      let deriving = self.parse_deriving_directive_list()
      let loc = self.loc_start_with(spos)
      TopTypeDef({
        tycon,
        tycon_loc,
        params: @list.new(),
        components,
        type_vis: vis,
        doc: DocString::empty(),
        deriving,
        loc,
        attrs,
        is_declare,
      })
    }
    TYPE => {
      self.expect_token(TK_TYPE)
      let tycon_loc = self.peek_location()
      let tycon = self.parse_luident()
      let params = self.parse_opt_type_params_no_constraints()
      let components : @syntax.TypeDesc = match self.peek_token() {
        SEMI(_) | EOF | DERIVE => Abstract
        EQUAL => {
          self.expect_token(TK_EQUAL)
          let typ = self.parse_type()
          Alias(typ)
        }
        _ => {
          let typ = self.parse_type()
          Newtype(typ)
        }
      }
      let deriving = self.parse_deriving_directive_list()
      let loc = self.loc_start_with(spos)
      TopTypeDef({
        tycon,
        tycon_loc,
        params,
        components,
        type_vis: vis,
        doc: DocString::empty(),
        deriving,
        loc,
        attrs,
        is_declare,
      })
    }
    TYPEALIAS => self.parse_typealias(spos~, attrs~, vis~, is_declare~)
    STRUCT => {
      self.expect_token(TK_STRUCT)
      let tycon_loc = self.peek_location()
      let tycon = self.parse_luident()
      let params = self.parse_opt_type_params_no_constraints()
      let components : @syntax.TypeDesc = match self.peek_token() {
        LPAREN => {
          // tuple struct
          self.skip() // consume '('
          let (ts, _) = self.series1_with_follow(
            delim=TK_COMMA,
            follow_set=[TK_RPAREN],
            State::parse_type,
          )
          self.expect_token(TK_RPAREN)
          TupleStruct(@list.from_array(ts))
        }
        _ => {
          let fields = self.surround_series(
            invalid_delims=[TK_COMMA],
            left=TK_LBRACE,
            right=TK_RBRACE,
            delim=TK_SEMI,
            State::parse_field_decl,
          )
          Record(@list.from_array(fields))
        }
      }
      let deriving = self.parse_deriving_directive_list()
      let loc = self.loc_start_with(spos)
      TopTypeDef({
        tycon,
        tycon_loc,
        params,
        components,
        type_vis: vis,
        doc: DocString::empty(),
        deriving,
        loc,
        attrs,
        is_declare,
      })
    }
    ENUM => {
      self.expect_token(TK_ENUM)
      let tycon_loc = self.peek_location()
      let tycon = self.parse_luident()
      let params = self.parse_opt_type_params_no_constraints()
      let constrs = self.surround_series(
          invalid_delims=[TK_COMMA],
          left=TK_LBRACE,
          right=TK_RBRACE,
          delim=TK_SEMI,
          State::parse_constr_decl,
        )
        |> @list.from_array
      let deriving = self.parse_deriving_directive_list()
      let loc = self.loc_start_with(spos)
      TopTypeDef({
        tycon,
        tycon_loc,
        params,
        components: Variant(constrs),
        type_vis: vis,
        doc: DocString::empty(),
        deriving,
        loc,
        attrs,
        is_declare,
      })
    }
    LET | CONST => {
      let (is_constant, binder) = match self.peek_token() {
        LET => {
          self.skip()
          (false, self.parse_binder(context="name of `let`"))
        }
        CONST => {
          self.skip()
          let loc = self.peek_location()
          let name = self.expect_uident(context="name of `const`")
          (true, { name, loc })
        }
        _ => panic()
      }
      let ty = self.parse_opt_annot()
      self.expect_token(TK_EQUAL)
      let expr = self.parse_expr()
      let loc = self.loc_start_with(spos)
      TopLetDef(
        binder~,
        ty~,
        expr~,
        is_constant~,
        vis~,
        loc~,
        doc=DocString::empty(),
        attrs~,
        is_declare~,
      )
    }
    EXTERN if self.peek_token(nth=1) is TYPE => {
      self..skip().skip()
      let tycon_loc = self.peek_location()
      let tycon = self.parse_luident()
      let params = self.parse_opt_type_params_no_constraints()
      let deriving = self.parse_deriving_directive_list()
      let loc = self.loc_start_with(spos)
      TopTypeDef({
        tycon,
        tycon_loc,
        params,
        components: Extern,
        type_vis: vis,
        doc: DocString::empty(),
        deriving,
        loc,
        attrs,
        is_declare,
      })
    }
    EXTERN => {
      self.expect_token(TK_EXTERN)
      let language = self.expect_string()
      let fun_decl = self.parse_fun_decl(attrs~)
      let fun_decl = { ..fun_decl, vis, }
      self.expect_token(TK_EQUAL)
      if self.peek_token() is MULTILINE_STRING(_) {
        let xs = []
        while self.peek_token() is MULTILINE_STRING(x) {
          self.skip()
          xs.push(x)
        }
        let loc = self.loc_start_with(spos)
        TopFuncDef(
          fun_decl~,
          decl_body=DeclStubs(
            Embedded(
              language=Some(language),
              code=CodeMultilineString(@list.from_array(xs)),
            ),
          ),
          loc~,
        )
      } else {
        let body = self.expect_string()
        if self.peek_token() is STRING(_) {
          let fname = self.expect_string()
          let loc = self.loc_start_with(spos)
          TopFuncDef(
            fun_decl~,
            decl_body=DeclStubs(
              Import(module_name=body, func_name=fname, language=Some(language)),
            ),
            loc~,
          )
        } else {
          let loc = self.loc_start_with(spos)
          TopFuncDef(
            fun_decl~,
            decl_body=DeclStubs(
              Embedded(language=Some(language), code=CodeString(body)),
            ),
            loc~,
          )
        }
      }
    }
    ASYNC =>
      match self.peek_token(nth=1) {
        TEST => {
          let is_async = Some(self.peek_location())
          self.skip()
          self.parse_test_block(attrs~, spos~, is_async~)
        }
        _ => self.parse_top_func(attrs~, vis~, spos~, is_declare~)
      }
    FN => self.parse_top_func(attrs~, vis~, spos~, is_declare~)
    FNALIAS => {
      self.skip()
      fn parse_type_name_opt(self : State, need_dot) -> @syntax.Label? {
        match (need_dot, self.peek_token()) {
          (true, DOT_LIDENT(name) | DOT_UIDENT(name))
          | (false, LIDENT(name) | UIDENT(name)) if self.peek_token(nth=1)
            is COLONCOLON => {
            let loc = self.peek_location()
            self.skip()
            let type_name : @syntax.Label = { name, loc }
            self.expect_token(TK_COLONCOLON)
            Some(type_name)
          }
          _ => None
        }
      }

      fn parse_target(self : State, need_dot) -> @syntax.AliasTarget {
        let target_loc = self.peek_location()
        let target_name = if need_dot {
          self.expect_dot_lident(context="target of `fnalias`")
        } else {
          self.expect_lident(context="target of `fnalias`")
        }
        if self.peek_token() is AS {
          self.skip()
          let binder = self.parse_binder(context="name of `fnalias`")
          let target : @syntax.Label? = Some({
            name: target_name,
            loc: target_loc,
          })
          { binder, target }
        } else {
          { binder: { name: target_name, loc: target_loc }, target: None }
        }
      }

      fn first_alias_target(need_dot) -> Array[TokenKind] {
        if need_dot {
          [TK_DOT_LPAREN, TK_DOT_LIDENT]
        } else {
          [TK_LPAREN, TK_LIDENT]
        }
      }

      fn parse_targets(
        self : State,
        need_dot,
      ) -> (List[@syntax.AliasTarget], Bool) {
        match self.peek_token() {
          DOT_LPAREN if need_dot => {
            let xs = self.surround_series(
                left=TK_DOT_LPAREN,
                right=TK_RPAREN,
                delim=TK_COMMA,
                state => parse_target(state, false),
              )
              |> @list.from_array
            (xs, true)
          }
          LPAREN if !need_dot => {
            let xs = self.surround_series(
                left=TK_LPAREN,
                right=TK_RPAREN,
                delim=TK_COMMA,
                state => parse_target(state, false),
              )
              |> @list.from_array
            (xs, true)
          }
          DOT_LIDENT(_) if need_dot => {
            let xs = @list.from_array([parse_target(self, need_dot)])
            (xs, false)
          }
          LIDENT(_) if !need_dot => {
            let xs = @list.from_array([parse_target(self, need_dot)])
            (xs, false)
          }
          other => {
            let loc = self.peek_location()
            self.report_unexpected(other, first_alias_target(need_dot), loc)
            self.panic() |> ignore
            (@list.new(), false)
          }
        }
      }

      let (pkg, type_name, targets, is_list) = match self.peek_token() {
        PACKAGE_NAME(pkg) => {
          let pkg : @syntax.Label = { name: pkg, loc: self.peek_location() }
          self.skip()
          let type_name = parse_type_name_opt(self, true)
          let (targets, is_list) = parse_targets(self, type_name is None)
          (Some(pkg), type_name, targets, is_list)
        }
        LIDENT(_) | UIDENT(_) | LPAREN => {
          let type_name = parse_type_name_opt(self, false)
          let (targets, is_list) = parse_targets(self, false)
          (None, type_name, targets, is_list)
        }
        other => {
          let loc = self.peek_location()
          self.report_unexpected(
            other,
            [TK_PACKAGE_NAME, TK_LIDENT, TK_UIDENT, TK_LPAREN],
            loc,
          )
          self.panic() |> ignore
          (None, None, @list.new(), false)
        }
      }
      let loc = self.loc_start_with(spos)
      TopFuncAlias(
        pkg~,
        type_name~,
        targets~,
        vis~,
        attrs~,
        is_list~,
        doc=DocString::empty(),
        loc~,
      )
    }
    TRAIT => {
      let trait_decl = self.parse_trait_decl(attrs~, vis~, is_declare~)
      let trait_decl = { ..trait_decl, vis, }
      @syntax.TopTrait(trait_decl)
    }
    TRAITALIAS => {
      self.skip()
      fn parse_target(
        self : State,
        target_name,
        target_loc,
      ) -> @syntax.AliasTarget {
        if self.peek_token() is AS {
          self.skip()
          let binder_loc = self.peek_location()
          let binder_name = self.parse_luident()
          {
            target: Some({ name: target_name, loc: target_loc }),
            binder: { name: binder_name, loc: binder_loc },
          }
        } else {
          { target: None, binder: { name: target_name, loc: target_loc } }
        }
      }

      match self.peek_token() {
        PACKAGE_NAME(pkg) => {
          let pkg_loc = self.peek_location()
          let pkg : @syntax.Label? = Some({ name: pkg, loc: pkg_loc })
          self.skip()
          let (is_list, targets) = match self.peek_token() {
            DOT_LPAREN => {
              let xs = self.surround_series(
                left=TK_DOT_LPAREN,
                right=TK_RPAREN,
                delim=TK_COMMA,
                _ => {
                  let target_loc = self.peek_location()
                  let target_name = self.parse_luident()
                  parse_target(self, target_name, target_loc)
                },
              )
              (true, @list.from_array(xs))
            }
            DOT_LIDENT(target_name) | DOT_UIDENT(target_name) => {
              let target_loc = self.peek_location()
              self.skip()
              let target = parse_target(self, target_name, target_loc)
              (false, @list.from_array([target]))
            }
            other => {
              let other_loc = self.peek_location()
              self.report_unexpected(
                other,
                [TK_DOT_LPAREN, TK_LIDENT, TK_UIDENT],
                other_loc,
              )
              self.panic() |> ignore
              (false, @list.new())
            }
          }
          let loc = self.loc_start_with(spos)
          TopBatchTraitAlias(
            pkg~,
            targets~,
            vis~,
            attrs~,
            is_list~,
            doc=DocString::empty(),
            loc~,
          )
        }
        LIDENT(name) | UIDENT(name) if self.peek_token(nth=1) is EQUAL => {
          self.skip()
          let binder_loc = self.peek_location()
          let binder : @syntax.Binder = { name, loc: binder_loc }
          self.expect_token(TK_EQUAL)
          let target = self.parse_type_name()
          let loc = self.loc_start_with(spos)
          let (pkg, target) : (@syntax.Label?, @syntax.Label) = {
            let loc = target.loc
            match target.name {
              Dot(pkg~, id~) => (Some({ name: pkg, loc }), { name: id, loc })
              Ident(name=id) => (None, { name: id, loc })
            }
          }
          TopBatchTraitAlias(
            pkg~,
            targets=@list.from_array([{ binder, target: Some(target) }]),
            vis~,
            loc~,
            attrs~,
            is_list=false,
            doc=DocString::empty(),
          )
        }
        LIDENT(target_name) | UIDENT(target_name) => {
          let target_loc = self.peek_location()
          self.skip()
          let target = parse_target(self, target_name, target_loc)
          let loc = self.loc_start_with(spos)
          TopBatchTraitAlias(
            pkg=None,
            targets=@list.from_array([target]),
            vis~,
            is_list=false,
            loc~,
            attrs~,
            doc=DocString::empty(),
          )
        }
        LPAREN => {
          let targets = self.surround_series(
              left=TK_LPAREN,
              right=TK_RPAREN,
              delim=TK_COMMA,
              _ => {
                let target_loc = self.peek_location()
                let target_name = self.parse_luident()
                parse_target(self, target_name, target_loc)
              },
            )
            |> @list.from_array
          let loc = self.loc_start_with(spos)
          TopBatchTraitAlias(
            pkg=None,
            targets~,
            vis~,
            is_list=true,
            loc~,
            attrs~,
            doc=DocString::empty(),
          )
        }
        other => {
          let other_loc = self.peek_location()
          self.report_unexpected(
            other,
            [TK_UIDENT, TK_LIDENT, TK_LPAREN, TK_PACKAGE_NAME],
            other_loc,
          )
          let loc = self.panic()
          TopExpr(
            expr=Hole(loc~, kind=Synthesized),
            is_main=false,
            local_types=@list.new(),
            is_async=None,
            loc~,
          )
        }
      }
    }
    TEST => self.parse_test_block(attrs~, spos~, is_async=None)
    IMPL => {
      self.skip()
      self.push_sync(TK_WITH)
      self.push_sync(TK_FOR)
      let quantifiers = self.parse_opt_typ_params()
      let trait_ = self.parse_type_name()
      self.pop_sync(TK_FOR)
      if self.mode is Panic(loc~, ..) {
        self.pop_sync(TK_WITH)
        return TopExpr(
          expr=Hole(loc~, kind=Synthesized),
          is_main=false,
          local_types=@list.new(),
          is_async=None,
          loc~,
        )
      }
      match self.peek_token() {
        FOR => {
          self.skip()
          let self_ty = self.parse_type()
          self.pop_sync(TK_WITH)
          if self.peek_token() is WITH {
            self.skip()
            self.push_sync(TK_LBRACE)
            let method_name = self.parse_binder(context="name of method")
            let has_error = self.parse_optional_bang()
            let params = self.surround_series(
                left=TK_LPAREN,
                right=TK_RPAREN,
                delim=TK_COMMA,
                State::parse_parameter,
              )
              |> @list.from_array
            let (ret_ty, err_ty) = self.parse_func_return_type()
            self.pop_sync(TK_LBRACE)
            let body : @syntax.DeclBody = if self.peek_token() is EQUAL {
              self.skip()
              let code = self.expect_string()
              DeclStubs(Embedded(language=None, code=CodeString(code)))
            } else {
              let (local_types, body) = self.parse_block_expr_with_local_types()
              DeclBody(expr=body, local_types~)
            }
            TopImpl(
              self_ty=Some(self_ty),
              trait_~,
              method_name~,
              has_error~,
              quantifiers~,
              params~,
              ret_ty~,
              err_ty~,
              vis~,
              body~,
              loc=self.loc_start_with(spos),
              doc=DocString::empty(),
              attrs~,
            )
          } else {
            TopImplRelation(
              self_ty~,
              trait_~,
              vis~,
              quantifiers~,
              loc=self.loc_start_with(spos),
              attrs~,
              doc=DocString::empty(),
              is_declare~,
            )
          }
        }
        WITH => {
          self.pop_sync(TK_WITH)
          self.skip()
          self.push_sync(TK_LBRACE)
          let method_name = self.parse_binder(context="name of method")
          let has_error = self.parse_optional_bang()
          let params = self.surround_series(
              left=TK_LPAREN,
              right=TK_RPAREN,
              delim=TK_COMMA,
              State::parse_parameter,
            )
            |> @list.from_array
          let (ret_ty, err_ty) = self.parse_func_return_type()
          self.pop_sync(TK_LBRACE)
          let body : @syntax.DeclBody = if self.peek_token() is EQUAL {
            self.skip()
            let code = self.expect_string()
            DeclStubs(Embedded(language=None, code=CodeString(code)))
          } else {
            let (local_types, body) = self.parse_block_expr_with_local_types()
            DeclBody(expr=body, local_types~)
          }
          TopImpl(
            self_ty=None,
            trait_~,
            method_name~,
            has_error~,
            quantifiers~,
            params~,
            ret_ty~,
            err_ty~,
            vis~,
            body~,
            loc=self.loc_start_with(spos),
            doc=DocString::empty(),
            attrs~,
          )
        }
        other => {
          self.pop_sync(TK_WITH)
          let other_loc = self.peek_location()
          self.report_unexpected(other, [TK_FOR, TK_WITH], other_loc)
          self.panic() |> ignore
          let loc = self.loc_start_with(spos)
          TopImpl(
            self_ty=None,
            trait_~,
            method_name={ name: "", loc: @syntax.no_location },
            has_error=None,
            quantifiers=@list.new(),
            params=@list.new(),
            ret_ty=None,
            err_ty=NoErrorType,
            vis~,
            body=DeclBody(
              expr=Hole(loc=@syntax.no_location, kind=Synthesized),
              local_types=@list.new(),
            ),
            loc~,
            doc=DocString::empty(),
            attrs~,
          )
        }
      }
    }
    ENUMVIEW => {
      self.expect_token(TK_ENUMVIEW)
      let quantifiers = self.parse_opt_typ_params()
      let view_type_loc = self.peek_location()
      let view_type_name = self.expect_uident(context="view type name")
      let view_constrs = self.surround_series(
          invalid_delims=[TK_COMMA],
          left=TK_LBRACE,
          right=TK_RBRACE,
          delim=TK_SEMI,
          State::parse_constr_decl,
        )
        |> @list.from_array
      self.expect_token(TK_FOR)
      let source_ty = self.parse_type()
      self.expect_token(TK_WITH)
      let view_func_name = self.parse_binder(context="name of view function")
      let params_spos = self.peek_spos()
      let parameters = self.surround_series(
          left=TK_LPAREN,
          right=TK_RPAREN,
          delim=TK_COMMA,
          State::parse_parameter,
        )
        |> @list.from_array
      let params_loc = self.loc_start_with(params_spos)
      let body = self.parse_block_expr()
      let loc = self.loc_start_with(spos)
      TopView(
        quantifiers~,
        source_ty~,
        view_type_name~,
        view_type_loc~,
        view_func_name~,
        parameters~,
        params_loc~,
        view_constrs~,
        body~,
        vis~,
        loc~,
        attrs~,
        doc=DocString::empty(),
      )
    }
    USING if !is_declare => {
      self.skip()
      let pkg : @syntax.Label = match self.peek_token() {
        PACKAGE_NAME(name) => {
          let loc = self.peek_location()
          self.skip()
          { name, loc }
        }
        otherwise => {
          let loc = self.peek_location()
          self.report_unexpected(otherwise, [TK_PACKAGE_NAME], loc)
          self.panic() |> ignore
          { name: "", loc }
        }
      }
      fn parse_alias_target(
        state : State,
        lower : Bool,
        context : String,
      ) -> @syntax.AliasTarget {
        let target_loc = state.peek_location()
        let target = if lower {
          state.expect_lident(context~)
        } else {
          state.expect_uident(context~)
        }
        match self.peek_token() {
          AS => {
            state.skip()
            let binder_loc = state.peek_location()
            let binder_name = if lower {
              state.expect_lident(context~)
            } else {
              state.expect_uident(context~)
            }
            @syntax.AliasTarget::{
              binder: { name: binder_name, loc: binder_loc },
              target: Some({ name: target, loc: target_loc }),
            }
          }
          _ =>
            @syntax.AliasTarget::{
              binder: { name: target, loc: target_loc },
              target: None,
            }
        }
      }

      let names = self.surround_series(
          left=TK_LBRACE,
          right=TK_RBRACE,
          delim=TK_COMMA,
          fn(state) {
            match state.peek_token() {
              LIDENT(_) => {
                let target = parse_alias_target(state, true, "value name")
                (target, @syntax.UsingKind::Value)
              }
              UIDENT(_) => {
                let target = parse_alias_target(state, false, "constant name")
                (target, @syntax.UsingKind::Value)
              }
              TYPE => {
                state.skip()
                let target = parse_alias_target(state, false, "type name")
                (target, @syntax.UsingKind::Type)
              }
              TRAIT => {
                state.skip()
                let target = parse_alias_target(state, false, "trait name")
                (target, @syntax.UsingKind::Trait)
              }
              other => {
                let other_loc = state.peek_location()
                state.report_unexpected(
                  other,
                  [TK_LIDENT, TK_UIDENT, TK_TYPE, TK_TRAIT],
                  other_loc,
                )
                let loc = state.panic()
                (
                  { binder: { name: "", loc }, target: None },
                  @syntax.UsingKind::Value,
                )
              }
            }
          },
        )
        |> @list.from_array()
      let loc = self.loc_start_with(spos)
      @syntax.Impl::TopUsing(
        pkg~,
        names~,
        vis~,
        attrs~,
        loc~,
        doc=DocString::empty(),
      )
    }
    other => {
      let other_loc = self.peek_location()
      self.report_unexpected(other, first_impl_, other_loc)
      let loc = self.panic()
      TopExpr(
        expr=Hole(loc~, kind=Synthesized),
        is_main=false,
        local_types=@list.new(),
        is_async=None,
        loc~,
      )
    }
  }
}

///|
fn State::parse_deriving_directive_list(
  self : Self,
) -> List[@syntax.DerivingDirective] {
  if self.mode is Panic(_) {
    return @list.new()
  }
  if self.peek_token() is DERIVE {
    self.skip()
    let directives = self.surround_series(
      left=TK_LPAREN,
      right=TK_RPAREN,
      delim=TK_COMMA,
      State::parse_deriving_directive,
    )
    @list.from_array(directives)
  } else {
    @list.new()
  }
}

///|
fn State::parse_deriving_directive(self : Self) -> @syntax.DerivingDirective {
  if self.mode is Panic(loc~, ..) {
    return {
      type_name: { name: @syntax.Ident(name=""), is_object: false, loc },
      args: @list.new(),
      loc,
    }
  }
  let spos = self.peek_spos()
  let type_name = self.parse_type_name()
  let args = if self.peek_token() is LPAREN {
    self.surround_series(
      left=TK_LPAREN,
      right=TK_RPAREN,
      delim=TK_COMMA,
      State::parse_argument,
    )
    |> @list.from_array
  } else {
    @list.new()
  }
  let loc = self.loc_start_with(spos)
  { type_name, args, loc }
}

///|
fn State::parse_field_decl(self : Self) -> @syntax.FieldDecl {
  if self.mode is Panic(loc~, ..) {
    return {
      name: { label: "?field_name", loc },
      ty: Any(loc~),
      mut_: false,
      vis: Default,
      doc: DocString::empty(),
      attrs: @list.empty(),
      loc,
    }
  }
  let spos = self.peek_spos()
  let attrs = self.parse_attributes()
  let vis = self.parse_visibility()
  let mut_ = if self.peek_token() is MUTABLE {
    self.expect_token(TK_MUTABLE)
    true
  } else {
    false
  }
  let name_loc = self.peek_location()
  let name = self.expect_lident(context="struct field name")
  self.expect_token(TK_COLON)
  let ty = self.parse_type()
  let loc = self.loc_start_with(spos)
  {
    name: { label: name, loc: name_loc },
    ty,
    mut_,
    vis,
    loc,
    attrs,
    doc: DocString::empty(),
  }
}

///|
fn State::parse_constr_decl(self : Self) -> @syntax.ConstrDecl {
  if self.mode is Panic(loc~, ..) {
    return {
      name: { name: "?constr-name", loc },
      args: None,
      tag: None,
      loc,
      attrs: @list.empty(),
      doc: @syntax.DocString::empty(),
    }
  }
  let spos = self.peek_spos()
  let attrs = self.parse_attributes()
  if self.peek_token() is (PRIV | PUB | READONLY) {
    let loc = self.peek_location()
    self.skip()
    self.report_error({
      loc,
      msg: "No individual visibility for enum constructor.",
    })
  }
  let constr_name_loc = self.peek_location()
  let name = self.expect_uident(context="constructor name")
  let constr_name : @syntax.ConstrName = { name, loc: constr_name_loc }
  let args = if self.peek_token() is LPAREN {
    let xs = self.surround_series(
      left=TK_LPAREN,
      right=TK_RPAREN,
      delim=TK_COMMA,
      fn(state) -> @syntax.ConstrParam {
        let mut_ = if state.peek_token() is MUTABLE {
          state.skip()
          true
        } else {
          false
        }
        // `label~ : Type`
        if state.peek_token() is POST_LABEL(name) {
          let label_loc = Location::trim_last_char(state.peek_location())
          let label : @syntax.Label = { name, loc: label_loc }
          state.skip()
          state.expect_token(TK_COLON)
          let ty = state.parse_type()
          { ty, mut_, label: Some(label) }
        } else {
          let ty = state.parse_type()
          { ty, mut_, label: None }
        }
      },
    )
    Some(@list.from_array(xs))
  } else {
    None
  }
  let tag = if self.peek_token() is EQUAL {
    self.skip()
    let tag_loc = self.peek_location()
    let tag = self.expect_int()
    Some((tag, tag_loc))
  } else {
    None
  }
  let loc = self.loc_start_with(spos)
  { name: constr_name, args, tag, loc, attrs, doc: DocString::empty() }
}

///|
fn State::parse_pattern(self : Self) -> @syntax.Pattern {
  if self.mode is Panic(loc~, ..) {
    return Any(loc~)
  }
  let spos = self.peek_spos()
  let mut pat = self.parse_or_pattern()
  while self.peek_token() is AS {
    self.skip()
    let alias_ = self.parse_binder(context="variable name")
    let loc = self.loc_start_with(spos)
    pat = Alias(pat~, alias_~, loc~)
  } nobreak {
    pat
  }
}

///|
fn State::parse_or_pattern(self : Self) -> @syntax.Pattern {
  if self.mode is Panic(loc~, ..) {
    return Any(loc~)
  }
  let spos = self.peek_spos()
  let pat = self.parse_range_pattern()
  // TODO: use stack-safe loop
  if self.peek_token() is BAR {
    self.skip()
    let pat2 = self.parse_or_pattern()
    let loc = self.loc_start_with(spos)
    Or(pat1=pat, pat2~, loc~)
  } else {
    pat
  }
}

///|
fn State::parse_range_pattern(self : Self) -> @syntax.Pattern {
  if self.mode is Panic(loc~, ..) {
    return Any(loc~)
  }
  let spos = self.peek_spos()
  let lhs = self.parse_simple_pattern()
  match self.peek_token() {
    RANGE_INCLUSIVE => {
      self.skip()
      let rhs = self.parse_simple_pattern()
      let loc = self.loc_start_with(spos)
      Range(lhs~, rhs~, kind=Inclusive, loc~)
    }
    RANGE_EXCLUSIVE => {
      self.skip()
      let rhs = self.parse_simple_pattern()
      let loc = self.loc_start_with(spos)
      Range(lhs~, rhs~, kind=Exclusive, loc~)
    }
    _ => lhs
  }
}

///|
fn State::parse_constr_pattern(self : Self) -> @syntax.Pattern {
  let spos = self.peek_spos()
  fn parse_constr_args() {
    self.surround_series(left=TK_LPAREN, right=TK_RPAREN, delim=TK_COMMA, fn(
      _,
    ) {
      if self.peek_token() is DOTDOT {
        let loc = self.peek_location()
        self.skip()
        ConstrArgRelatedness::Dotdot(loc)
      } else {
        let args = self.parse_constr_pat_argument()
        ConstrArgRelatedness::Arg(args)
      }
    })
    |> @list.from_array
  }

  fn parse_constr_args_no_open() {
    self.surround_series(left=TK_LPAREN, right=TK_RPAREN, delim=TK_COMMA, _ => {
      self.parse_constr_pat_argument()
    })
    |> @list.from_array
  }

  match self.peek_token() {
    // bitstring constr pattern `i32(args)`
    // To disambiguate with pattern variable, the args list must be present.
    LIDENT(name) if self.peek_token(nth=1) is LPAREN => {
      self.skip() // skip lident
      let name_loc = self.peek_location()
      let binder : @syntax.Binder = { name, loc: name_loc }
      let args = parse_constr_args_no_open()
      let loc = self.loc_start_with(spos)
      @syntax.Pattern::SpecialConstr(binder~, args~, loc~)
    }
    PACKAGE_NAME(_) | UIDENT(_) | LIDENT(_) => {
      let constr = self.parse_constr()
      let maybe_args = self.optional([TK_LPAREN], _ => parse_constr_args())
      let mut is_open = false
      fn check_args(args : List[ConstrArgRelatedness]) {
        match args {
          Empty => @list.empty()
          More(Dotdot(_), tail=Empty) => {
            is_open = true
            @list.empty()
          }
          More(Dotdot(loc), tail~) => {
            is_open = true
            self.report_error({
              loc,
              msg: "Unexpected `..` here, add `, ..` behind the last argument to ignore the rest arguments.",
            })
            check_args(tail)
          }
          More(Arg(arg), tail~) => check_args(tail).add(arg)
        }
      }

      let args = maybe_args.map(check_args)
      let loc = self.loc_start_with(spos)
      Constr(constr~, args~, is_open~, loc~)
    }
    _ => panic()
  }
}

///|
fn State::parse_simple_pattern(self : Self) -> @syntax.Pattern {
  if self.mode is Panic(loc~, ..) {
    return Any(loc~)
  }
  match self.peek_token() {
    TRUE => {
      let loc = self.peek_location()
      self.expect_token(TK_TRUE)
      Constant(c=Bool(true), loc~)
    }
    FALSE => {
      let loc = self.peek_location()
      self.expect_token(TK_FALSE)
      Constant(c=Bool(false), loc~)
    }
    CHAR(x) => {
      let loc = self.peek_location()
      self.expect_token(TK_CHAR)
      Constant(c=Char(x), loc~)
    }
    INT(x) => {
      let loc = self.peek_location()
      self.expect_token(TK_INT)
      Constant(c=@syntax.make_int(x), loc~)
    }
    BYTE(x) => {
      let loc = self.peek_location()
      self.expect_token(TK_BYTE)
      Constant(c=Byte(x), loc~)
    }
    FLOAT(x) => {
      let loc = self.peek_location()
      self.expect_token(TK_FLOAT)
      Constant(c=@syntax.make_float(x), loc~)
    }
    DOUBLE(x) => {
      let loc = self.peek_location()
      self.expect_token(TK_DOUBLE)
      Constant(c=@syntax.make_double(x), loc~)
    }
    MINUS => {
      self.expect_token(TK_MINUS)
      match self.peek_token() {
        INT(x) => {
          let loc = self.peek_location()
          self.expect_token(TK_INT)
          Constant(c=@syntax.make_int("-" + x), loc~)
        }
        FLOAT(x) => {
          let loc = self.peek_location()
          self.expect_token(TK_FLOAT)
          Constant(c=@syntax.make_float("-" + x), loc~)
        }
        DOUBLE(x) => {
          let loc = self.peek_location()
          self.expect_token(TK_DOUBLE)
          Constant(c=@syntax.make_double("-" + x), loc~)
        }
        other => {
          let other_loc = self.peek_location()
          self.report_unexpected(
            other,
            [TK_INT, TK_FLOAT, TK_DOUBLE],
            other_loc,
          )
          let loc = self.panic()
          Any(loc~)
        }
      }
    }
    BYTES(x) => {
      let loc = self.peek_location()
      self.expect_token(TK_BYTES)
      Constant(c=Bytes(x), loc~)
    }
    STRING(x) => {
      let loc = self.peek_location()
      self.expect_token(TK_STRING)
      Constant(c=String(x), loc~)
    }
    UNDERSCORE => {
      let loc = self.peek_location()
      self.expect_token(TK_UNDERSCORE)
      Any(loc~)
    }
    LIDENT(_) if !(self.peek_token(nth=1) is (COLONCOLON | LPAREN)) => {
      let binder = self.parse_binder(context="")
      Var(binder)
    }
    PACKAGE_NAME(_) | UIDENT(_) | LIDENT(_) => self.parse_constr_pattern()
    LPAREN => {
      let spos = self.peek_spos()
      let result = self.surround(left=TK_LPAREN, right=TK_RPAREN, fn(state) {
        let first = state.parse_pattern()
        match state.peek_token() {
          COMMA => {
            state.skip()
            let (pats, _) = state.series_with_follow(
              delim=TK_COMMA,
              follow_set=[TK_RPAREN],
              State::parse_pattern,
            )
            if pats is [] {
              first
            } else {
              Tuple(
                pats=@list.from_array([first, ..pats]),
                loc=state.loc_start_with(spos),
              )
            }
          }
          COLON => {
            state.skip()
            let ty = state.parse_type()
            @syntax.Pattern::Constraint(
              pat=first,
              ty~,
              loc=state.loc_start_with(spos),
            )
          }
          RPAREN => first
          other => {
            let other_loc = state.peek_location()
            state.report_unexpected(
              other,
              [TK_COMMA, TK_COLON, TK_RPAREN],
              other_loc,
            )
            state.panic() |> ignore
            first
          }
        }
      })
      result
    }
    LBRACKET => {
      let spos = self.peek_spos()
      let pats1 = []
      let pats2 = []
      let mut dotdot = NoDotDot
      fn push_pat(pat : @syntax.ArrayPattern) {
        if dotdot is NoDotDot {
          pats1.push(pat)
        } else {
          pats2.push(pat)
        }
      }

      let _ = self.surround_series(
        left=TK_LBRACKET,
        right=TK_RBRACKET,
        delim=TK_COMMA,
        fn(self) {
          if self.peek_token() is DOTDOT {
            let pat_spos = self.peek_spos()
            self.skip()
            match self.peek_token() {
              STRING(str) => {
                let loc = self.peek_location()
                self.skip()
                push_pat(StringSpread(str~, loc~))
              }
              BYTES(bytes) => {
                let loc = self.peek_location()
                self.skip()
                push_pat(BytesSpread(bytes~, loc~))
              }
              UIDENT(name) => {
                let id_loc = self.peek_location()
                self.skip()
                let binder : @syntax.Binder = { name, loc: id_loc }
                let loc = self.loc_start_with(pat_spos)
                push_pat(ConstSpread(binder~, pkg=None, loc~))
              }
              PACKAGE_NAME(pkg) => {
                self.skip()
                let id_loc = self.peek_location()
                let name = self.expect_dot_uident(context="name of constant")
                let binder : @syntax.Binder = { name, loc: id_loc }
                let loc = self.loc_start_with(pat_spos)
                push_pat(ConstSpread(binder~, pkg=Some(pkg), loc~))
              }
              _ =>
                match dotdot {
                  NoBinder | WithBinder(_) | Underscore | WithAsBinder(_) =>
                    self.report_error({
                      loc: self.peek_location(),
                      msg: "At most one `..` is allowed in array pattern.",
                    })
                  NoDotDot =>
                    dotdot = match self.peek_token() {
                      LIDENT(_) => {
                        let binder = self.parse_binder(context="")
                        WithBinder(binder)
                      }
                      AS(_) => {
                        self.skip()
                        let binder = self.parse_binder(context="variable name")
                        WithAsBinder(binder)
                      }
                      UNDERSCORE => {
                        self.skip()
                        Underscore
                      }
                      _ => NoBinder
                    }
                }
            }
          } else {
            push_pat(Pattern(self.parse_pattern()))
          }
        },
      )
      let pats1 = @list.from_array(pats1)
      let pats2 = @list.from_array(pats2)
      let pats : @syntax.ArrayPatterns = match dotdot {
        NoDotDot => Closed(pats1)
        NoBinder => Open(pats1, pats2, NoBinder)
        WithBinder(b) => Open(pats1, pats2, Binder(b))
        Underscore => Open(pats1, pats2, Underscore)
        WithAsBinder(b) => Open(pats1, pats2, BinderAs(b))
      }
      let loc = self.loc_start_with(spos)
      Array(pats~, loc~)
    }
    LBRACE => {
      let spos = self.peek_spos()
      let mut kind : BraceLikeSyntaxKind = Unknown
      self.expect_token(TK_LBRACE)
      self.push_sync(TK_RBRACE)
      let (field_pat_or_dotdots, _) = self.series_with_follow(
        delim=TK_COMMA,
        follow_set=[TK_RBRACE],
        fn(self) {
          let spos = self.peek_spos()
          fn parse_map_elem_pat(key) {
            if kind is Unknown {
              kind = Map
            }
            self.skip() // skip the token for constant
            let key_loc = self.loc_start_with(spos)
            let match_absent = if self.peek_token() is QUESTION {
              self.skip()
              true
            } else {
              false
            }
            self.expect_token(TK_COLON)
            let pat = self.parse_pattern()
            let loc = self.loc_start_with(spos)
            MapPatternFragment::MapElem({ key, pat, match_absent, key_loc, loc })
          }

          match self.peek_token() {
            DOTDOT => {
              let loc = self.peek_location()
              self.skip()
              MapPatternFragment::Dotdot(loc)
            }
            TRUE => parse_map_elem_pat(Bool(true))
            FALSE => parse_map_elem_pat(Bool(false))
            CHAR(x) => parse_map_elem_pat(Char(x))
            INT(x) => parse_map_elem_pat(@syntax.make_int(x))
            BYTE(x) => parse_map_elem_pat(Byte(x))
            FLOAT(x) => parse_map_elem_pat(@syntax.make_float(x))
            DOUBLE(x) => parse_map_elem_pat(@syntax.make_double(x))
            MINUS => {
              self.expect_token(TK_MINUS)
              match self.peek_token() {
                INT(x) => parse_map_elem_pat(@syntax.make_int("-" + x))
                FLOAT(x) => parse_map_elem_pat(@syntax.make_float("-" + x))
                DOUBLE(x) => parse_map_elem_pat(Double("-" + x))
                other => {
                  let other_loc = self.peek_location()
                  self.report_failed_to_parse(
                    other, "`-Int` or `-Float` in key of map pattern", other_loc,
                  )
                  self.panic() |> ignore
                  MapPatternFragment::Error
                }
              }
            }
            BYTES(x) => parse_map_elem_pat(Bytes(x))
            STRING(x) => parse_map_elem_pat(String(x))
            LIDENT(_) => {
              if kind is Unknown {
                kind = Record
              }
              MapPatternFragment::Field(self.parse_field_pat())
            }
            other => {
              let expected : Array[TokenKind] = match kind {
                Unknown => [TK_LIDENT, ..first_map_pattern_key]
                Map => first_map_pattern_key
                Record => [TK_LIDENT]
              }
              self.report_unexpected(other, expected, self.peek_location())
              self.panic() |> ignore
              MapPatternFragment::Error
            }
          }
        },
      )
      self.pop_sync(TK_RBRACE)
      self.expect_token(TK_RBRACE)
      let loc = self.loc_start_with(spos)
      match kind {
        Unknown | Record => {
          // record pattern
          fn collect_fields(
            has_dotdot,
            acc : List[@syntax.FieldPat],
            field_or_dotdots : ArrayView[MapPatternFragment],
          ) -> @syntax.Pattern {
            match field_or_dotdots {
              [] => Record(fields=acc.rev(), is_closed=!has_dotdot, loc~)
              [Dotdot(loc), .. rest] => {
                if !(rest is []) {
                  self.report_error({
                    loc,
                    msg: "Unexpected `..` here, add `, ..` behind the last field to ignore the rest of record.",
                  })
                }
                collect_fields(true, acc, rest)
              }
              [Field(field_pat), .. rest] =>
                collect_fields(has_dotdot, acc.add(field_pat), rest)
              [MapElem({ loc, .. }), .. rest] => {
                self.report_error({
                  loc,
                  msg: "Record pattern and map pattern cannot be mixed",
                })
                collect_fields(has_dotdot, acc, rest)
              }
              [Error, .. rest] => collect_fields(has_dotdot, acc, rest)
            }
          }

          collect_fields(false, @list.new(), field_pat_or_dotdots)
        }
        Map => {
          // Error only appears when we fail to parse a contant pattern,
          // so it's fine to assume it is a map pattern in this case
          fn collect_elems(
            has_dotdot,
            acc : List[_],
            field_pat_or_dotdots : ArrayView[MapPatternFragment],
          ) -> @syntax.Pattern {
            match field_pat_or_dotdots {
              [] => Map(elems=acc.rev(), is_closed=!has_dotdot, loc~)
              [Dotdot(loc), .. rest] => {
                if !(rest is []) {
                  self.report_error({ loc, msg: "Unexpected `..` here" })
                }
                collect_elems(true, acc, rest)
              }
              [Field({ loc, .. }), .. rest] => {
                self.report_error({ loc, msg: "Unexpected `..` here" })
                collect_elems(has_dotdot, acc, rest)
              }
              [Error, .. rest] => collect_elems(has_dotdot, acc, rest)
              [MapElem(elem), .. rest] =>
                collect_elems(has_dotdot, acc.add(elem), rest)
            }
          }

          collect_elems(false, @list.new(), field_pat_or_dotdots)
        }
      }
    }
    other => {
      let other_loc = self.peek_location()
      self.report_unexpected(other, first_simple_pattern, other_loc)
      let loc = self.panic()
      Any(loc~)
    }
  }
}

///|
priv enum BraceLikeSyntaxKind {
  Map
  Record
  Unknown
}

///|
fn State::parse_label(self : Self, context~ : String) -> @syntax.Label {
  if self.mode is Panic(loc~, ..) {
    return { name: "?lident", loc }
  }
  let loc = self.peek_location()
  let name = self.expect_lident(context~)
  { name, loc }
}

///|
fn State::parse_field_pat(self : Self) -> @syntax.FieldPat {
  if self.mode is Panic(loc~, ..) {
    return {
      label: { name: "?ident", loc },
      pattern: Any(loc~),
      is_pun: false,
      loc,
    }
  }
  let spos = self.peek_spos()
  let label = self.parse_label(context="struct field")
  if self.peek_token() is COLON {
    self.skip()
    let pattern = self.parse_pattern()
    let loc = self.loc_start_with(spos)
    { label, pattern, is_pun: false, loc }
  } else {
    let loc = self.loc_start_with(spos)
    let pattern : @syntax.Pattern = Var({ name: label.name, loc: label.loc })
    { label, pattern, is_pun: true, loc }
  }
}

///|
fn State::parse_constr_pat_argument(self : Self) -> @syntax.ConstrPatArg {
  if self.mode is Panic(loc~, ..) {
    return { pat: Any(loc~), kind: Positional }
  }
  match self.peek_token() {
    // label~
    POST_LABEL(label_name) => {
      let label : @syntax.Label = {
        name: label_name,
        loc: self.peek_location(),
      }
      self.skip()
      let (pat, kind) : (_, @syntax.ArgumentKind) = match self.peek_token() {
        EQUAL => {
          self.skip()
          let pat = self.parse_pattern()
          self.report_error({
            loc: label.loc,
            msg: "unexpected `~` in argument. Did you mean `label=pattern` or `label~`?",
          })
          (pat, Labelled(label))
        }
        COMMA | RPAREN => {
          let loc = Location::trim_last_char(label.loc)
          let pat = @syntax.label_to_pat(loc~, label)
          (pat, LabelledPun(label))
        }
        _ => {
          let loc = self.peek_location()
          (Any(loc~), Labelled(label))
        }
      }
      { pat, kind }
    }
    LIDENT(name) | UIDENT(name) if self.peek_token(nth=1) is EQUAL => {
      let label : @syntax.Label = { name, loc: self.peek_location() }
      self.skip() // luident
      self.expect_token(TK_EQUAL)
      let pat = self.parse_pattern()
      { pat, kind: Labelled(label) }
    }
    _ => {
      let pat = self.parse_pattern()
      { pat, kind: Positional }
    }
  }
}

///|
priv enum Assocativity {
  Left
  Right
}

///|
/// suppress the `Right is never used` warning
test {
  let r = Right
  match r {
    Left => ()
    Right => ()
  }
}

///|
priv enum ConstrArgRelatedness {
  Dotdot(Location)
  Arg(@syntax.ConstrPatArg)
}

///|
priv enum MapPatternFragment {
  MapElem(@syntax.MapPatElem)
  Field(@syntax.FieldPat)
  Dotdot(Location)
  Error
}

///|
priv enum ArrayPatternDotDot {
  NoBinder
  WithBinder(@syntax.Binder)
  Underscore
  WithAsBinder(@syntax.Binder)
  NoDotDot
}

///|
fn is_binop(tok : Token) -> Bool {
  match tok {
    BARBAR
    | AMPERAMPER
    | BAR
    | CARET
    | AMPER
    | INFIX1(_)
    | INFIX2(_)
    | PLUS
    | MINUS
    | INFIX3(_)
    | INFIX4(_) => true
    _ => false
  }
}

///|
fn get_binop_prior_assoc(tok : Token) -> (Int, Assocativity) {
  match tok {
    BARBAR => (1, Right)
    AMPERAMPER => (2, Right)
    BAR => (4, Left)
    CARET => (5, Left)
    AMPER => (6, Left)
    INFIX1(_) => (7, Left)
    INFIX2(_) => (8, Left)
    PLUS | MINUS => (9, Left)
    INFIX3(_) => (10, Left)
    INFIX4(_) => (11, Left)
    _ => abort("not a binop!")
  }
}

///|
fn State::parse_binop(self : Self) -> @syntax.Var {
  if self.mode is Panic(loc~, ..) {
    return { name: Ident(name="?binop"), loc }
  }
  match self.peek_token() {
    BARBAR => {
      let loc = self.peek_location()
      self.expect_token(TK_BARBAR)
      { name: Ident(name="||"), loc }
    }
    AMPERAMPER => {
      let loc = self.peek_location()
      self.expect_token(TK_AMPERAMPER)
      { name: Ident(name="&&"), loc }
    }
    BAR => {
      let loc = self.peek_location()
      self.expect_token(TK_BAR)
      { name: Ident(name="|"), loc }
    }
    CARET => {
      let loc = self.peek_location()
      self.expect_token(TK_CARET)
      { name: Ident(name="^"), loc }
    }
    AMPER => {
      let loc = self.peek_location()
      self.expect_token(TK_AMPER)
      { name: Ident(name="&"), loc }
    }
    INFIX1(x) => {
      let loc = self.peek_location()
      self.expect_token(TK_INFIX1)
      { name: Ident(name=x), loc }
    }
    INFIX2(x) => {
      let loc = self.peek_location()
      self.expect_token(TK_INFIX2)
      { name: Ident(name=x), loc }
    }
    PLUS => {
      let loc = self.peek_location()
      self.expect_token(TK_PLUS)
      { name: Ident(name="+"), loc }
    }
    MINUS => {
      let loc = self.peek_location()
      self.expect_token(TK_MINUS)
      { name: Ident(name="-"), loc }
    }
    INFIX3(x) => {
      let loc = self.peek_location()
      self.expect_token(TK_INFIX3)
      { name: Ident(name=x), loc }
    }
    INFIX4(x) => {
      let loc = self.peek_location()
      self.expect_token(TK_INFIX4)
      { name: Ident(name=x), loc }
    }
    other => {
      let other_loc = self.peek_location()
      self.report_failed_to_parse(other, "binary operator", other_loc)
      let loc = self.panic()
      { name: Ident(name="?binop"), loc }
    }
  }
}

///|
fn State::parse_loop_expr(self : Self, label~ : @syntax.Label?) -> @syntax.Expr {
  if self.mode is Panic(loc~, ..) {
    return Hole(loc~, kind=Synthesized)
  }
  let spos = self.peek_spos()
  self.skip() // skip LOOP token
  let arg = self.with_follow(follow_set=[TK_LBRACE], fn(_) { self.parse_expr() })
  let loop_loc = self.loc_start_with(spos)
  let body = match self.mode {
    Normal =>
      self.surround_series(
        invalid_delims=[TK_COMMA],
        left=TK_LBRACE,
        right=TK_RBRACE,
        delim=TK_SEMI,
        fn(_) { self.parse_case() },
      )
      |> @list.from_array
    Panic(_) => @list.new()
  }
  let loc = self.loc_start_with(spos)
  Loop(arg~, body~, label~, loop_loc~, loc~)
}

///|
fn State::parse_where_clause(self : State) -> @syntax.WhereClause? {
  if self.peek_token() is WHERE {
    let spos = self.peek_spos()
    self.skip()
    self.expect_token(TK_LBRACE)
    let (fields, _) = self.series_with_follow(
      delim=TK_COMMA,
      follow_set=[TK_RBRACE],
      State::parse_field_def,
    )
    let fields = @list.from_array(fields)
    self.expect_token(TK_RBRACE)
    let loc = self.loc_start_with(spos)
    Some({ fields, loc })
  } else {
    None
  }
}

///|
fn State::parse_for_expr(self : Self, label~ : @syntax.Label?) -> @syntax.Expr {
  if self.mode is Panic(loc~, ..) {
    return Hole(loc~, kind=Synthesized)
  }
  let spos = self.peek_spos()
  self.skip() // skip FOR token
  fn handle_error(expected : Array[TokenKind]) -> @syntax.Expr {
    let loc = self.peek_location()
    let token = self.peek_token()
    self.report_unexpected(token, expected, loc)
    // If the loop header is incorrect,
    // but there is a loop body there,
    // currently our error recovery (which does not balance parenthesis)
    // may give bad result.
    // So try to recover a loop body here. 
    //
    // This pattern is equivalent to skip_until(LBRACE). There is
    // always an invariant that "if the next token is not expected, it's in panic mode."
    // Why do we still need to keep track of the panic and sync states if there is a simpler approach?
    self.push_sync(TK_LBRACE)
    self.panic() |> ignore
    self.pop_sync(TK_LBRACE)
    let body = self.parse_block_expr()
    let loc = self.loc_start_with(spos)
    For(
      binders=@list.new(),
      condition=None,
      continue_block=@list.new(),
      body~,
      for_else=None,
      label~,
      loc~,
      where_clause=None,
    )
  }

  match self.peek_token() {
    SEMI(_) | LBRACE => self.parse_for_loop(spos~, label~)
    UNDERSCORE | IN =>
      // The case of `in` is actually syntax-incorrect,
      // but it is very likely a `for .. in` loop,
      // so we treat it as `for .. in` for better error message & recovery.
      self.parse_foreach_loop(spos~, label~)
    LIDENT(_) =>
      match self.peek_token(nth=1) {
        EQUAL => self.parse_for_loop(spos~, label~)
        COMMA | IN => self.parse_foreach_loop(spos~, label~)
        _ => {
          self.skip()
          handle_error([TK_EQUAL, TK_COMMA, TK_IN])
        }
      }
    _ => handle_error([TK_LIDENT, TK_SEMI, TK_LBRACE])
  }
}

///|
fn State::parse_foreach_loop(
  self : Self,
  spos~ : Position,
  label~ : @syntax.Label?,
) -> @syntax.Expr {
  if self.mode is Panic(loc~, ..) {
    return Hole(loc~, kind=Synthesized)
  }
  let binders = self.with_follow(follow_set=[TK_IN], fn(state) {
    state.sepby1(delim=TK_COMMA, fn(state) -> @syntax.Binder? {
      match state.peek_token() {
        UNDERSCORE => {
          state.skip()
          None
        }
        LIDENT(name) => {
          let loc = state.peek_location()
          state.skip()
          Some({ name, loc })
        }
        other => {
          let loc = state.peek_location()
          state.report_unexpected(other, [TK_LIDENT, TK_UNDERSCORE], loc)
          state.panic() |> ignore
          None
        }
      }
    })
    |> @list.from_array
  })
  self.expect_token(TK_IN)
  let expr = self.parse_expr()
  let body = self.parse_block_expr()
  let else_block = if self.peek_token() is ELSE {
    self.skip()
    Some(self.parse_block_expr())
  } else {
    None
  }
  let loc = self.loc_start_with(spos)
  ForEach(binders~, expr~, body~, else_block~, loc~, label~)
}

///|
fn State::parse_for_loop(
  self : Self,
  spos~ : Position,
  label~ : @syntax.Label?,
) -> @syntax.Expr {
  if self.mode is Panic(loc~, ..) {
    return Hole(loc~, kind=Synthesized)
  }
  fn parse_binder_with_value(state : State) -> (@syntax.Binder, @syntax.Expr) {
    let binder = state.parse_binder(context="loop variable name of `for`")
    state.expect_token(TK_EQUAL)
    let value = state.parse_expr()
    (binder, value)
  }

  let (binders, _) = self.series_with_follow(
    delim=TK_COMMA,
    follow_set=[TK_SEMI, TK_LBRACE],
    parse_binder_with_value,
  )
  let binders = @list.from_array(binders)
  let (condition, continue_block) = match self.peek_token() {
    LBRACE => (None, @list.new())
    SEMI(_) => {
      self.push_sync(TK_LBRACE)
      self.expect_token(TK_SEMI)
      let condition = self.optional(first_expr, State::try_parse_condition_expr)
      self.expect_token(TK_SEMI)
      let (continue_block, _) = self.series_with_follow(
        delim=TK_COMMA,
        follow_set=[TK_LBRACE],
        parse_binder_with_value,
      )
      let continue_block = @list.from_array(continue_block)
      self.pop_sync(TK_LBRACE)
      (condition, continue_block)
    }
    other => {
      let other_loc = self.peek_location()
      self.report_unexpected(other, [TK_SEMI, TK_LBRACE], other_loc)
      (None, @list.new())
    }
  }
  let body = self.parse_block_expr()
  let for_else = if self.peek_token() is ELSE {
    self.with_follow(follow_set=[TK_LBRACE], fn(state) {
      state.expect_token(TK_ELSE)
    })
    Some(self.parse_block_expr())
  } else {
    None
  }
  let where_clause = self.parse_where_clause()
  let loc = self.loc_start_with(spos)
  For(
    binders~,
    condition~,
    continue_block~,
    body~,
    for_else~,
    label~,
    loc~,
    where_clause~,
  )
}

///|
fn State::parse_while_loop(
  self : Self,
  label~ : @syntax.Label?,
) -> @syntax.Expr {
  if self.mode is Panic(loc~, ..) {
    return Hole(loc~, kind=Synthesized)
  }
  let spos = self.peek_spos()
  self.expect_token(TK_WHILE)
  let loop_cond = self.with_follow(
    follow_set=[TK_LBRACE],
    State::try_parse_condition_expr,
  )
  let loop_body = self.parse_block_expr()
  let while_else = if self.peek_token() is ELSE {
    self.with_follow(follow_set=[TK_LBRACE], fn(state) {
      state.expect_token(TK_ELSE)
    })
    Some(self.parse_block_expr())
  } else {
    None
  }
  let loc = self.loc_start_with(spos)
  While(loop_cond~, loop_body~, while_else~, loc~, label~)
}

///|
fn State::try_parse_pipe_expr(self : Self) -> @syntax.Expr {
  if self.mode is Panic(loc~, ..) {
    return Hole(loc~, kind=Synthesized)
  }
  match self.peek_token() {
    // an error production to handle `try invalid-pipe-expr`
    IF
    | MATCH
    | LOOP
    | WHILE
    | FOR
    | TRY
    | LET
    | GUARD
    | BREAK
    | RETURN
    | CONTINUE =>
      self.handle_unexpected_expr_or_statement(context="the try body")
    _ => self.parse_pipe_expr(parsed=None)
  }
}

///|
fn State::parse_try_expr(self : Self) -> @syntax.Expr {
  if self.mode is Panic(loc~, ..) {
    return Hole(loc~, kind=Synthesized)
  }
  let spos = self.peek_spos()
  let try_loc = self.peek_location()
  self.expect_token(TK_TRY)
  let body = self.try_parse_pipe_expr()
  let catch_start = self.peek_spos()
  self.expect_token(TK_CATCH)
  let catch_all = if self.peek_token() is EXCLAMATION {
    self.skip()
    true
  } else {
    false
  }
  let lbrace_epos = self.peek_epos()
  // the catch_loc is the location of `catch {` or `catch! {`
  let catch_loc : Location = { start: catch_start, end: lbrace_epos }
  let catch_cases = self.surround_series(
      invalid_delims=[TK_COMMA],
      left=TK_LBRACE,
      right=TK_RBRACE,
      delim=TK_SEMI,
      State::parse_case,
    )
    |> @list.from_array
  let (try_else, else_loc) = if self.peek_token() is (ELSE | NORAISE) {
    let else_spos = self.peek_spos()
    self.skip()
    let lbrace_epos = self.peek_epos()
    // the else_loc is the location of `else {` or `noraise {`
    let else_loc : Location = { start: else_spos, end: lbrace_epos }
    let try_else_cases = self.surround_series(
        invalid_delims=[TK_COMMA],
        left=TK_LBRACE,
        right=TK_RBRACE,
        delim=TK_SEMI,
        State::parse_case,
      )
      |> @list.from_array
    (Some(try_else_cases), else_loc)
  } else {
    (None, @syntax.no_location)
  }
  let loc = self.loc_start_with(spos)
  Try(
    body~,
    catch_=catch_cases,
    catch_all~,
    try_else~,
    has_try=true,
    try_loc~,
    catch_loc~,
    else_loc~,
    loc~,
  )
}

///|
fn State::parse_try_question(self : Self) -> @syntax.Expr {
  if self.mode is Panic(loc~, ..) {
    return Hole(loc~, kind=Synthesized)
  }
  let spos = self.peek_spos()
  let try_loc = self.peek_location()
  self.skip()
  let body = self.parse_pipe_expr(parsed=None)
  let loc = self.loc_start_with(spos)
  TryOperator(body~, kind=Question, try_loc~, loc~)
}

///|
fn State::parse_try_exclamation(self : Self) -> @syntax.Expr {
  if self.mode is Panic(loc~, ..) {
    return Hole(loc~, kind=Synthesized)
  }
  let spos = self.peek_spos()
  let try_loc = self.peek_location()
  self.skip()
  let body = self.parse_pipe_expr(parsed=None)
  let loc = self.loc_start_with(spos)
  TryOperator(body~, kind=Exclamation, try_loc~, loc~)
}

///|
fn State::parse_if_expr(self : Self) -> @syntax.Expr {
  if self.mode is Panic(loc~, ..) {
    return Hole(loc~, kind=Synthesized)
  }
  let spos = self.peek_spos()
  self.expect_token(TK_IF)
  let cond = self.with_follow(
    follow_set=[TK_LBRACE],
    State::try_parse_condition_expr,
  )
  let ifso = self.parse_block_expr()
  let ifnot = if self.peek_token() is ELSE {
    self.skip()
    Some(
      if self.peek_token() is IF {
        self.parse_expr() // TODO: call parse_if_expr directly
      } else {
        self.parse_block_expr()
      },
    )
  } else {
    None
  }
  let loc = self.loc_start_with(spos)
  If(cond~, ifso~, ifnot~, loc~)
}

///|
fn State::parse_match_expr(self : Self) -> @syntax.Expr {
  if self.mode is Panic(loc~, ..) {
    return Hole(loc~, kind=Synthesized)
  }
  let spos = self.peek_spos()
  self.expect_token(TK_MATCH)
  let expr = self.with_follow(follow_set=[TK_LBRACE], fn(_) {
    self.try_parse_condition_expr()
  })
  let lbrace_epos = self.peek_epos()
  // The main_loc is the location of `match expr {`
  let match_loc : Location = { start: spos, end: lbrace_epos }
  let cases = self.surround_series(
      invalid_delims=[TK_COMMA],
      left=TK_LBRACE,
      right=TK_RBRACE,
      delim=TK_SEMI,
      State::parse_case,
    )
    |> @list.from_array
  let loc = self.loc_start_with(spos)
  Match(expr~, cases~, match_loc~, loc~)
}

///|
fn State::parse_case(self : Self) -> @syntax.Case {
  if self.mode is Panic(loc~, ..) {
    return {
      pattern: Any(loc~),
      guard_: None,
      body: Hole(loc~, kind=Synthesized),
    }
  }
  if self.peek_token() is ELLIPSIS {
    let loc = self.peek_location()
    self.skip()
    { pattern: Any(loc~), guard_: None, body: Hole(loc~, kind=Todo) }
  } else {
    let pat = self.with_syncs([TK_FAT_ARROW, TK_IF], State::parse_pattern)
    let guard_ = if self.peek_token() is IF {
      self.skip()
      let expr = match self.peek_token() {
        // an error production to handle `pattern if invalid-infix-expr => action`
        IF
        | MATCH
        | LOOP
        | WHILE
        | FOR
        | TRY
        | LET
        | GUARD
        | BREAK
        | RETURN
        | CONTINUE =>
          self.handle_unexpected_expr_or_statement(context="pattern guard")
        _ => self.parse_infix_expr(parsed=None)
      }
      Some(expr)
    } else {
      None
    }
    let body = self.parse_fat_arrow_action()
    { pattern: pat, guard_, body }
  }
}

///|
fn State::parse_fat_arrow_action(self : State) -> @syntax.Expr {
  self.expect_token(TK_FAT_ARROW)
  match self.peek_token() {
    // an error production to handle `pattern => invalid-expr-statement`
    LET | LETREC | GUARD | DEFER =>
      self.handle_unexpected_expr_or_statement(
        context="the action part of a matching case",
      )
    _ => self.parse_expr_stmt()
  }
}

///|
fn State::parse_opt_with_strategy(self : State) -> @syntax.Label? {
  match self.peek_token() {
    WITH => {
      self.skip()
      let label = self.parse_label(context="strategy of `lexmatch`")
      Some(label)
    }
    _ => None
  }
}

///|
fn State::parse_lexmatch_expr(self : State) -> @syntax.Expr {
  let spos = self.peek_spos()
  self.expect_token(TK_LEXMATCH)
  let (expr, strategy) = self.with_follow(follow_set=[TK_LBRACE], fn(state) {
    let expr = state.try_parse_condition_expr()
    let strategy = state.parse_opt_with_strategy()
    (expr, strategy)
  })
  let match_loc = @basic.Location::{ start: spos, end: self.peek_epos() }
  fn parse_body(state : State) -> @list.List[@syntax.LexCase] {
    let (cases, _) = state.series_with_follow(
      invalid_delims=[TK_COMMA],
      delim=TK_SEMI,
      follow_set=[TK_RBRACE],
      State::parse_lexmatch_case,
    )
    @list.from_array(cases)
  }

  let cases = self.surround(left=TK_LBRACE, right=TK_RBRACE, parse_body)
  let loc = self.loc_start_with(spos)
  @syntax.Expr::LexMatch(strategy~, expr~, cases~, match_loc~, loc~)
}

///|
fn State::parse_lexmatch_case(self : State) -> @syntax.LexCase {
  match self.peek_token() {
    ELLIPSIS => {
      let loc = self.peek_location()
      self.skip()
      @syntax.LexCase::{
        pat: @list.singleton(Wildcard(loc~)),
        pat_loc: loc,
        body: Hole(loc~, kind=Todo),
      }
    }
    _ => {
      let (pat, pat_loc) = self.with_syncs(
        [TK_FAT_ARROW, TK_COMMA],
        State::parse_lex_pattern,
      )
      let act = self.parse_fat_arrow_action()
      { pat, pat_loc, body: act }
    }
  }
}

///|
fn State::parse_lex_pattern(
  self : State,
) -> (@list.List[@syntax.LexTopPattern], @basic.Location) {
  match self.peek_token() {
    LPAREN => {
      let spos = self.peek_spos()
      self.skip()
      let pats = self.with_follow(follow_set=[TK_RPAREN], state => {
          state.sepby1(delim=TK_COMMA, State::parse_lex_top_pattern)
        })
        |> @list.from_array
      self.expect_token(TK_RPAREN)
      let loc = self.loc_start_with(spos)
      (pats, loc)
    }
    LIDENT(_) => {
      let loc = self.peek_location()
      let name = self.expect_lident(context="lex rest binder")
      (@list.singleton(Binder({ name, loc })), loc)
    }
    UNDERSCORE => {
      let loc = self.peek_location()
      self.skip()
      (@list.singleton(Wildcard(loc~)), loc)
    }
    REGEX_LITERAL(_) | REGEX_INTERP(_) | STRING(_) | INTERP(_) => {
      let spos = self.peek_spos()
      let pat = self.parse_lex_simple_atom_pattern()
      (@list.singleton(Pattern(pat)), self.loc_start_with(spos))
    }
    other => {
      let other_loc = self.peek_location()
      self.report_failed_to_parse(other, "lex pattern", other_loc)
      let loc = self.panic()
      (@list.singleton(Wildcard(loc~)), loc)
    }
  }
}

///|
fn State::parse_lex_top_pattern(self : State) -> @syntax.LexTopPattern {
  match self.peek_token() {
    LIDENT(_) => {
      let loc = self.peek_location()
      let name = self.expect_lident(context="lex rest binder")
      Binder({ name, loc })
    }
    UNDERSCORE(_) => {
      let loc = self.peek_location()
      self.skip()
      Wildcard(loc~)
    }
    _ => {
      let pat = self.parse_lex_as_pattern()
      Pattern(pat)
    }
  }
}

///|
fn State::parse_lex_as_pattern(self : State) -> @syntax.LexPattern {
  let spos = self.peek_spos()
  let pats = self.parse_lex_pattern_sequence()
  match pats {
    More(pat, tail=Empty) if self.peek_token() is AS => {
      self.skip()
      let binder = self.parse_binder(context="binder in `as` pattern")
      let loc = self.loc_start_with(spos)
      Alias(pat~, binder~, loc~)
    }
    More(pat, tail=Empty) => return pat
    _ => {
      let loc = self.loc_start_with(spos)
      Sequence(pats~, loc~)
    }
  }
}

///|
fn State::parse_lex_pattern_sequence(
  self : State,
) -> @list.List[@syntax.LexPattern] {
  let first = self.parse_lex_atom_pattern()
  let pats = [first]
  loop self.peek_token() {
    SEMI(_)
    | REGEX_LITERAL(_)
    | REGEX_INTERP(_)
    | STRING(_)
    | INTERP(_)
    | LPAREN as tok => {
      if tok is SEMI(_) {
        self.skip()
      }
      let pat = self.parse_lex_atom_pattern()
      pats.push(pat)
      continue self.peek_token()
    }
    _ => ()
  }
  @list.from_array(pats)
}

///|
fn State::parse_lex_atom_pattern(self : State) -> @syntax.LexPattern {
  match self.peek_token() {
    REGEX_LITERAL(_) | REGEX_INTERP(_) | STRING(_) | INTERP(_) =>
      self.parse_lex_simple_atom_pattern()
    LPAREN => {
      self.skip()
      let alias_spos = self.peek_spos()
      let pats = self.with_syncs([TK_RPAREN], State::parse_lex_pattern_sequence)
      match pats {
        More(pat, tail=Empty) =>
          match self.peek_token() {
            AS => {
              self.skip()
              let binder = self.parse_binder(context="binder in `as` pattern")
              let loc = self.loc_start_with(alias_spos)
              self.expect_token(TK_RPAREN)
              Alias(pat~, binder~, loc~)
            }
            _ => {
              self.expect_token(TK_RPAREN)
              pat
            }
          }
        _ => {
          self.expect_token(TK_RPAREN)
          Sequence(pats~, loc=self.peek_location())
        }
      }
    }
    other => {
      let other_loc = self.peek_location()
      self.report_failed_to_parse(other, "lex atom pattern", other_loc)
      let loc = self.panic()
      Sequence(pats=@list.empty(), loc~)
    }
  }
}

///|
fn State::parse_lex_simple_atom_pattern(self : State) -> @syntax.LexPattern {
  match self.peek_token() {
    REGEX_LITERAL(lit) => {
      let loc = self.peek_location()
      self.skip()
      Regex(lit~, loc~)
    }
    REGEX_INTERP(elems) => {
      let loc = self.peek_location()
      self.skip()
      let elems = @syntax.make_interps(elems)
      RegexInterp(elems~, loc~)
    }
    STRING(lit) => {
      let loc = self.peek_location()
      self.skip()
      Regex(lit~, loc~)
    }
    INTERP(elems) => {
      let loc = self.peek_location()
      self.skip()
      let elems = @syntax.make_interps(elems)
      RegexInterp(elems~, loc~)
    }
    _ => panic()
  }
}

///|
fn State::parse_tuple_or_constraint(self : Self) -> @syntax.Expr {
  if self.mode is Panic(loc~, ..) {
    return Hole(loc~, kind=Synthesized)
  }
  let spos = self.peek_spos()
  self.expect_token(TK_LPAREN)
  self.push_sync(TK_RPAREN)
  let state = self
  let first = state.optional(first_expr, State::parse_expr)
  match first {
    None => {
      self.expect_token(TK_RPAREN)
      self.pop_sync(TK_RPAREN)
      let loc = state.loc_start_with(spos)
      @syntax.Expr::Unit(loc~, faked=false)
    }
    Some(first) =>
      match state.peek_token() {
        COLON => {
          state.expect_token(TK_COLON)
          let ty = state.parse_type()
          self.expect_token(TK_RPAREN)
          self.pop_sync(TK_RPAREN)
          let loc = state.loc_start_with(spos)
          Constraint(expr=first, ty~, loc~)
        }
        COMMA => {
          state.expect_token(TK_COMMA)
          let (rest, _) = state.series_with_follow(
            delim=TK_COMMA,
            follow_set=[TK_RPAREN],
            State::parse_expr,
          )
          self.expect_token(TK_RPAREN)
          self.pop_sync(TK_RPAREN)
          let loc = state.loc_start_with(spos)
          if rest.length() == 0 {
            Group(expr=first, group=Paren, loc~)
          } else {
            let rest = @list.from_array(rest)
            Tuple(exprs=rest.add(first), loc~)
          }
        }
        RPAREN => {
          self.expect_token(TK_RPAREN)
          self.pop_sync(TK_RPAREN)
          let loc = state.loc_start_with(spos)
          Group(expr=first, group=Paren, loc~)
        }
        _ => first //TODO: error recovery and add group node
      }
  }
}

///|
fn State::handle_lparen(self : Self) -> @syntax.Expr {
  let spos = self.peek_spos()
  self.expect_token(TK_LPAREN)
  let (expr_or_binders, has_trailing) = self.series_with_follow(
    delim=TK_COMMA,
    follow_set=[TK_RPAREN],
    State::parse_expr_or_binder,
  )
  self.expect_token(TK_RPAREN)
  let handle_expr = fn(expr) { self.parse_simple_try_expr(parsed=Some(expr)) }
  match (self.peek_token(), expr_or_binders) {
    (FAT_ARROW, _) => {
      let params_loc = self.loc_start_with(spos)
      self.skip()
      fn as_binder(expr : @syntax.Expr) -> @syntax.ArrowFnParam {
        match expr {
          Ident(id={ name: Ident(name~), .. }, loc~) => Named({ name, loc })
          Hole(kind=Incomplete, loc~) => Unnamed(loc)
          // error recovery
          Hole(kind=Synthesized, loc~) => Unnamed(loc)
          expr => {
            let loc = expr.loc()
            self.report_error({
              loc,
              msg: "Unexpected expression. You may expect parameter here.",
            })
            Unnamed(loc)
          }
        }
      }

      let parameters = expr_or_binders.map(x => {
          match x {
            Binder(expr, colon_loc=_, ty~) => (as_binder(expr), Some(ty))
            Expr(expr) => (as_binder(expr), None)
          }
        })
        |> @list.from_array
      let body = self.parse_expr_stmt_no_break_continue_return()
      let loc = self.loc_start_with(spos)
      Function(
        loc~,
        func=@syntax.make_arrow_fn(params_loc~, loc~, parameters, body),
      )
    }
    (_, []) => {
      let loc = self.loc_start_with(spos)
      handle_expr(Unit(loc~, faked=false))
    }
    (_, [Binder(expr, ty~, colon_loc=_)]) if !has_trailing => {
      let loc = self.loc_start_with(spos)
      handle_expr(Constraint(expr~, ty~, loc~))
    }
    (_, [Expr(expr)]) => {
      let loc = self.loc_start_with(spos)
      handle_expr(Group(expr~, group=Paren, loc~))
    }
    (_, _) => {
      let exprs = expr_or_binders.map(x => {
          match x {
            Binder(expr, colon_loc~, ty=_) => {
              self.report_unexpected(COLON, [TK_COMMA, TK_RPAREN], colon_loc)
              @syntax.Hole(kind=Synthesized, loc=expr.loc())
            }
            Expr(expr) => expr
          }
        })
        |> @list.from_array
      let loc = self.loc_start_with(spos)
      handle_expr(Tuple(exprs~, loc~))
    }
  }
}

///|
fn State::parse_expr_or_binder(self : Self) -> ExprOrBinderFragment {
  if self.mode is Panic(loc~, ..) {
    return Expr(Hole(kind=Synthesized, loc~))
  }
  let expr = self.parse_expr()
  if self.peek_token() is COLON {
    let colon_loc = self.peek_location()
    self.skip()
    let ty = self.parse_type()
    Binder(expr, colon_loc~, ty~)
  } else {
    Expr(expr)
  }
}

///|
fn State::parse_expr(self : Self) -> @syntax.Expr {
  if self.mode is Panic(loc~, ..) {
    return Hole(loc~, kind=Synthesized)
  }
  match self.peek_token() {
    POST_LABEL(label_name) => {
      let label : @syntax.Label = {
        name: label_name,
        loc: self.peek_location(),
      }
      self.skip()
      self.expect_token(TK_COLON)
      match self.peek_token() {
        LOOP => self.parse_loop_expr(label=Some(label))
        FOR => self.parse_for_expr(label=Some(label))
        WHILE => self.parse_while_loop(label=Some(label))
        other => {
          let loc = self.peek_location()
          self.report_unexpected(other, [TK_LOOP, TK_FOR, TK_WHILE], loc)
          self.panic() |> ignore
          Hole(loc~, kind=Synthesized)
        }
      }
    }
    LEXMATCH => self.parse_lexmatch_expr()
    LOOP => self.parse_loop_expr(label=None)
    FOR => self.parse_for_expr(label=None)
    WHILE => self.parse_while_loop(label=None)
    TRY => self.parse_try_expr()
    TRY_QUESTION => self.parse_try_question()
    TRY_EXCLAMATION => self.parse_try_exclamation()
    IF => self.parse_if_expr()
    MATCH => self.parse_match_expr()
    LPAREN => self.handle_lparen()
    LIDENT(_) | UNDERSCORE if self.peek_token(nth=1) is FAT_ARROW => {
      let spos = self.peek_spos()
      let p : @syntax.ArrowFnParam = match self.peek_token() {
        LIDENT(_) => {
          let binder = self.parse_binder(context="")
          Named(binder)
        }
        UNDERSCORE => {
          let loc = self.peek_location()
          self.expect_token(TK_UNDERSCORE)
          Unnamed(loc)
        }
        _ => panic()
      }
      let params_loc = self.loc_start_with(spos)
      self.expect_token(TK_FAT_ARROW)
      let body = self.parse_expr_stmt_no_break_continue_return()
      let loc = self.loc_start_with(spos)
      let func = @syntax.make_arrow_fn(
        @list.from_array([(p, None)]),
        body,
        params_loc~,
        loc~,
      )
      Function(loc~, func~)
    }
    _ =>
      match self.try_parse_arrow_fn_expr() {
        Err(parsed) => self.parse_simple_try_expr(parsed~)
        Ok(expr) => expr
      }
  }
}

///|
fn State::parse_simple_try_expr(
  self : Self,
  parsed~ : @syntax.Expr?,
) -> @syntax.Expr {
  if self.mode is Panic(loc~, ..) {
    return Hole(loc~, kind=Synthesized)
  }
  let spos = self.peek_spos()
  let e = self.parse_pipe_expr(parsed~)
  match self.peek_token() {
    CATCH => {
      let catch_start = self.peek_spos()
      self.skip()
      let catch_all = if self.peek_token() is EXCLAMATION {
        self.skip()
        true
      } else {
        false
      }
      let catch_loc = self.loc_start_with(catch_start)
      let catch_cases = self.surround_series(
          left=TK_LBRACE,
          right=TK_RBRACE,
          delim=TK_SEMI,
          State::parse_case,
        )
        |> @list.from_array
      let loc = self.loc_start_with(spos)
      Try(
        body=e,
        catch_=catch_cases,
        catch_all~,
        try_else=None,
        has_try=false,
        try_loc=catch_loc,
        catch_loc~,
        else_loc=@syntax.no_location,
        loc~,
      )
    }
    _ => e
  }
}

///|
fn State::try_parse_arrow_fn_expr(
  self : State,
) -> Result[@syntax.Expr, @syntax.Expr?] {
  match self.peek_token() {
    LPAREN => {
      let spos = self.peek_spos()
      self.skip()
      let (expr_or_binders, has_trailing) = self.series_with_follow(
        delim=TK_COMMA,
        follow_set=[TK_RPAREN],
        State::parse_expr_or_binder,
      )
      self.expect_token(TK_RPAREN)
      let params_loc = self.loc_start_with(spos)
      match (self.peek_token(), expr_or_binders) {
        (FAT_ARROW, _) => {
          self.skip()
          let parameters = expr_or_binders.map(fn(expr_or_binder) {
            let as_binder = fn(expr : @syntax.Expr) -> @syntax.ArrowFnParam {
              match expr {
                Ident(id={ name: Ident(name~), .. }, loc~) =>
                  Named({ name, loc })
                Hole(kind=Incomplete, loc~) => Unnamed(loc)
                Hole(kind=Synthesized, loc~) => Unnamed(loc)
                expr => {
                  let loc = expr.loc()
                  self.report_error({
                    loc,
                    msg: "Parse error, unexpected expression, you may expect parameter",
                  })
                  Unnamed(loc)
                }
              }
            }
            match expr_or_binder {
              Binder(expr, ty~, colon_loc=_) => (as_binder(expr), Some(ty))
              Expr(expr) => (as_binder(expr), None)
            }
          })
          let body = self.parse_expr_stmt_no_break_continue_return()
          let loc = self.loc_start_with(spos)
          Ok(
            @syntax.Expr::Function(
              func=@syntax.make_arrow_fn(
                params_loc~,
                loc~,
                @list.from_array(parameters),
                body,
              ),
              loc~,
            ),
          )
        }
        (_, []) => {
          let loc = self.loc_start_with(spos)
          Err(Some(@syntax.Expr::Unit(loc~, faked=false)))
        }
        (_, [Binder(expr, ty~, colon_loc=_)]) if !has_trailing => {
          let loc = self.loc_start_with(spos)
          Err(Some(@syntax.Expr::Constraint(expr~, ty~, loc~)))
        }
        (_, [Expr(expr)]) => {
          let loc = self.loc_start_with(spos)
          Err(Some(@syntax.Group(expr~, group=Paren, loc~)))
        }
        (_, _) => {
          let exprs = expr_or_binders.map(fn(expr_or_binder) {
            match expr_or_binder {
              Binder(expr, colon_loc~, ty=_) => {
                self.report_unexpected(COLON, [TK_COMMA, TK_RPAREN], colon_loc)
                @syntax.Expr::Hole(kind=Synthesized, loc=expr.loc())
              }
              Expr(expr) => expr
            }
          })
          let loc = self.loc_start_with(spos)
          Err(Some(@syntax.Expr::Tuple(exprs=@list.from_array(exprs), loc~)))
        }
      }
    }
    LIDENT(_) | UNDERSCORE if self.peek_token(nth=1) is FAT_ARROW => {
      let spos = self.peek_spos()
      let p : @syntax.ArrowFnParam = match self.peek_token() {
        LIDENT(_) => Named(self.parse_binder(context=""))
        UNDERSCORE => {
          self.expect_token(TK_UNDERSCORE)
          Unnamed(self.peek_location())
        }
        _ => panic()
      }
      let params_loc = self.loc_start_with(spos)
      self.expect_token(TK_FAT_ARROW)
      let body = self.parse_expr_stmt_no_break_continue_return()
      let loc = self.loc_start_with(spos)
      Ok(
        @syntax.Expr::Function(
          loc~,
          func=@syntax.make_arrow_fn(
            params_loc~,
            loc~,
            @list.singleton((p, None)),
            body,
          ),
        ),
      )
    }
    _ => Err(None)
  }
}

///|
fn State::parse_pipe_expr(self : Self, parsed~ : @syntax.Expr?) -> @syntax.Expr {
  if self.mode is Panic(loc~, ..) {
    return Hole(loc~, kind=Synthesized)
  }
  let spos = self.peek_spos_with_parsed(parsed~)
  let mut lhs = self.parse_infix_expr(parsed~)
  while self.peek_token() is PIPE {
    self.skip()
    let rhs = if self.peek_token() is LIDENT(binder) &&
      self.peek_token(nth=1) is FAT_ARROW {
      let binder_loc = self.peek_location()
      let spos = self.peek_spos()
      self.skip()
      self.skip()
      let body = if self.peek_token() is LBRACE {
        self.parse_block_expr()
      } else {
        let loc = self.peek_location()
        self.report_failed_to_parse(self.peek_token(), "`{ ... }`", loc)
        self.parse_infix_expr(parsed=None)
      }
      let loc = self.loc_start_with(spos)
      let binder = @syntax.Binder::{ name: binder, loc: binder_loc }
      let func = @syntax.make_arrow_fn(
        params_loc=binder_loc,
        loc~,
        @list.singleton((Named(binder), None)),
        body,
      )
      @syntax.Expr::Function(func~, loc~)
    } else {
      match self.try_parse_arrow_fn_expr() {
        Ok(fn_expr) => {
          // error recover: `(x) => e`, `e1 |> (x,y) => e2` or `e1 |> (x : Int) => e2`
          let loc = fn_expr.loc()
          ignore(loc)
          // self.report_unexpected_structure(found="form of anonymous function", expected="`x => { ... }`", loc~)
          fn_expr
        }
        Err(parsed) =>
          // allow `lhs |> infix_expr`, the typer will check infix_expr in a later stage.
          self.parse_infix_expr(parsed~)
      }
    }
    let loc = self.loc_start_with(spos)
    lhs = Pipe(lhs~, rhs~, loc~)
  }
  lhs
}

///|
fn State::parse_infix_expr(
  self : Self,
  parsed~ : @syntax.Expr?,
) -> @syntax.Expr {
  if self.mode is Panic(loc~, ..) {
    return Hole(loc~, kind=Synthesized)
  }
  fn make_infix(lpos, lhs, op, rhs) -> (Position, @syntax.Expr) {
    let loc = self.loc_start_with(lpos)
    (lpos, Infix(lhs~, op~, rhs~, loc~))
  }

  fn push_infix(
    expr_stack : List[(Position, @syntax.Expr)],
    op_stack : List[(Token, @syntax.Var)],
    next_op : Token,
  ) -> (List[(Position, @syntax.Expr)], List[(Token, @syntax.Var)]) {
    match (expr_stack, op_stack) {
      (_, Empty) => (expr_stack, op_stack)
      (More(rhs, tail=More(lhs, tail=expr_stack2)), More(op, tail=op_stack2)) => {
        let (priority1, assoc1) = get_binop_prior_assoc(op.0)
        let (priority2, _) = get_binop_prior_assoc(next_op)
        if priority1 > priority2 || (priority1 == priority2 && assoc1 is Left) {
          let new_expr = make_infix(lhs.0, lhs.1, op.1, rhs.1)
          push_infix(expr_stack2.add(new_expr), op_stack2, next_op)
        } else {
          (expr_stack, op_stack)
        }
      }
      _ => panic()
    }
  }

  fn reduce_current_stack(
    expr_stack : List[(Position, @syntax.Expr)],
    op_stack : List[(Token, @syntax.Var)],
  ) -> (Position, @syntax.Expr) {
    match (expr_stack, op_stack) {
      (More(expr, ..), Empty) => expr
      (More(rhs, tail=More(lhs, tail=expr_stack2)), More(op, tail=op_stack2)) => {
        let new_expr = make_infix(lhs.0, lhs.1, op.1, rhs.1)
        reduce_current_stack(expr_stack2.add(new_expr), op_stack2)
      }
      _ => panic()
    }
  }

  fn go(
    expr_stack : List[(Position, @syntax.Expr)],
    op_stack : List[(Token, @syntax.Var)],
  ) -> (Position, @syntax.Expr) {
    if self.mode is Panic(_) {
      reduce_current_stack(expr_stack, op_stack)
    } else {
      let tok = self.peek_token()
      if !is_binop(tok) {
        reduce_current_stack(expr_stack, op_stack)
      } else {
        let (expr_stack, op_stack) = push_infix(expr_stack, op_stack, tok)
        let var_ = self.parse_binop()
        let rhs_spos = self.peek_spos()
        let new_rhs = self.parse_postfix_expr(parsed=None)
        go(expr_stack.add((rhs_spos, new_rhs)), op_stack.add((tok, var_)))
      }
    }
  }

  let first_spos = self.peek_spos_with_parsed(parsed~)
  let first_expr = self.parse_postfix_expr(parsed~)
  let (_, expr) = go(@list.from_array([(first_spos, first_expr)]), @list.new())
  expr
}

///|
fn State::parse_postfix_expr(
  self : Self,
  parsed~ : @syntax.Expr?,
) -> @syntax.Expr {
  if self.mode is Panic(loc~, ..) {
    return Hole(loc~, kind=Synthesized)
  }
  let spos = self.peek_spos_with_parsed(parsed~)
  let expr = self.parse_range_expr(parsed~)
  match self.peek_token() {
    AS => {
      self.skip()
      let trait_ = self.parse_type_name()
      let loc = self.loc_start_with(spos)
      As(expr~, trait_~, loc~)
    }
    IS => {
      self.skip()
      let pat = self.parse_range_pattern()
      let loc = self.loc_start_with(spos)
      Is(expr~, pat~, loc~)
    }
    LEXMATCH_QUESTION => {
      self.skip()
      let (pat, pat_loc) = self.parse_lex_pattern()
      let strategy = self.parse_opt_with_strategy()
      IsLexMatch(
        expr~,
        pat~,
        pat_loc~,
        strategy~,
        loc=self.loc_start_with(spos),
      )
    }
    _ => expr
  }
}

///|
fn State::parse_range_expr(
  self : Self,
  parsed~ : @syntax.Expr?,
) -> @syntax.Expr {
  if self.mode is Panic(loc~, ..) {
    return Hole(loc~, kind=Synthesized)
  }
  let spos = self.peek_spos_with_parsed(parsed~)
  let lhs = self.parse_prefix_expr(parsed~)
  let op_start = self.peek_spos()
  match self.peek_token() {
    RANGE_INCLUSIVE => {
      self.skip()
      let op_loc = self.loc_start_with(op_start)
      let rhs = self.parse_prefix_expr(parsed=None)
      let loc = self.loc_start_with(spos)
      let op : @syntax.Var = { name: Ident(name="..="), loc: op_loc }
      Infix(op~, lhs~, rhs~, loc~)
    }
    RANGE_LT_INCLUSIVE => {
      self.skip()
      let op_loc = self.loc_start_with(op_start)
      let rhs = self.parse_prefix_expr(parsed=None)
      let loc = self.loc_start_with(spos)
      let op : @syntax.Var = { name: Ident(name="..<="), loc: op_loc }
      @syntax.Expr::Infix(op~, lhs~, rhs~, loc~)
    }
    RANGE_EXCLUSIVE => {
      self.skip()
      let op_loc = self.loc_start_with(op_start)
      let rhs = self.parse_prefix_expr(parsed=None)
      let loc = self.loc_start_with(spos)
      let op : @syntax.Var = { name: Ident(name="..<"), loc: op_loc }
      Infix(op~, lhs~, rhs~, loc~)
    }
    RANGE_INCLUSIVE_REV => {
      self.skip()
      let op_loc = self.loc_start_with(op_start)
      let rhs = self.parse_prefix_expr(parsed=None)
      let loc = self.loc_start_with(spos)
      let op : @syntax.Var = { name: Ident(name=">=.."), loc: op_loc }
      @syntax.Expr::Infix(op~, lhs~, rhs~, loc~)
    }
    RANGE_EXCLUSIVE_REV => {
      self.skip()
      let op_loc = self.loc_start_with(op_start)
      let rhs = self.parse_prefix_expr(parsed=None)
      let loc = self.loc_start_with(spos)
      let op : @syntax.Var = { name: Ident(name=">.."), loc: op_loc }
      @syntax.Expr::Infix(op~, lhs~, rhs~, loc~)
    }
    _ => lhs
  }
}

///|
fn State::parse_prefix_expr(
  self : Self,
  parsed~ : @syntax.Expr?,
) -> @syntax.Expr {
  if self.mode is Panic(loc~, ..) {
    return Hole(loc~, kind=Synthesized)
  }
  let spos = self.peek_spos()
  match self.peek_token() {
    PLUS if parsed is None => {
      self.expect_token(TK_PLUS)
      let expr = self.parse_prefix_expr(parsed=None)
      let loc = self.loc_start_with(spos)
      @syntax.make_uplus(loc~, "+", expr)
    }
    MINUS if parsed is None => {
      self.expect_token(TK_MINUS)
      let expr = self.parse_prefix_expr(parsed=None)
      let loc = self.loc_start_with(spos)
      @syntax.make_uminus(loc~, "-", expr)
    }
    EXCLAMATION => {
      self.expect_token(TK_EXCLAMATION)
      let expr = self.parse_prefix_expr(parsed=None)
      let loc = self.loc_start_with(spos)
      @syntax.make_unot(loc~, expr)
    }
    _ => {
      fn go(left : @syntax.Expr) -> @syntax.Expr {
        match self.peek_token() {
          EXCLAMATION | QUESTION | LPAREN => {
            let attr = self.parse_apply_attr()
            let args = self.surround_series(
                left=TK_LPAREN,
                right=TK_RPAREN,
                delim=TK_COMMA,
                State::parse_argument,
              )
              |> @list.from_array
            let loc = self.loc_start_with(spos)
            go(Apply(func=left, args~, attr~, loc~))
          }
          LBRACKET => {
            fn parse_index() {
              match self.peek_token() {
                COLON => {
                  self.skip()
                  let end_index = self.optional(first_expr, State::parse_expr)
                  SliceIndex(None, end_index)
                }
                _ => {
                  let start_index = self.parse_expr()
                  match self.peek_token() {
                    COLON => {
                      self.skip()
                      let end_index = self.optional(
                        first_expr,
                        State::parse_expr,
                      )
                      SliceIndex(Some(start_index), end_index)
                    }
                    _ => ArrayIndex(start_index)
                  }
                }
              }
            }

            let index_spos = self.peek_spos()
            let index = self.surround(left=TK_LBRACKET, right=TK_RBRACKET, _ => {
              parse_index()
            })
            let loc = self.loc_start_with(index_spos)
            let index_loc = self.loc_start_with(index_spos)
            let array_get_expr : @syntax.Expr = match index {
              ArrayIndex(index) => ArrayGet(loc~, array=left, index~)
              SliceIndex(start_index, end_index) =>
                ArrayGetSlice(
                  loc~,
                  index_loc~,
                  array=left,
                  start_index~,
                  end_index~,
                )
            }
            go(array_get_expr)
          }
          DOT_LIDENT(name) => {
            let label_loc = self.peek_location()
            let label : @syntax.Label = { name, loc: label_loc }
            self.skip()
            let attr = self.parse_apply_attr()
            match self.peek_token() {
              LPAREN => {
                let args = self.surround_series(
                    left=TK_LPAREN,
                    right=TK_RPAREN,
                    delim=TK_COMMA,
                    State::parse_argument,
                  )
                  |> @list.from_array
                let loc = self.loc_start_with(spos)
                let expr : @syntax.Expr = DotApply(
                  self=left,
                  method_name=label,
                  args~,
                  return_self=false,
                  attr~,
                  loc~,
                )
                go(expr)
              }
              _ => {
                let loc = self.loc_start_with(spos)
                let accessor : @syntax.Accessor = if label.name == "_" {
                  Newtype(loc~)
                } else {
                  Label(label)
                }
                go(Field(record=left, accessor~, loc~))
              }
            }
          }
          DOTDOT => {
            self.skip()
            let label = self.parse_label(context="method name")
            let attr = self.parse_apply_attr()
            let args = self.surround_series(
                left=TK_LPAREN,
                right=TK_RPAREN,
                delim=TK_COMMA,
                State::parse_argument,
              )
              |> @list.from_array
            let loc = self.loc_start_with(spos)
            let expr : @syntax.Expr = DotApply(
              self=left,
              method_name=label,
              args~,
              return_self=true,
              attr~,
              loc~,
            )
            go(expr)
          }
          DOT_INT(_) => {
            let accessor_loc = self.peek_location()
            let tuple_index = self.expect_dot_int()
            let accessor : @syntax.Accessor = Index(
              tuple_index~,
              loc=accessor_loc,
            )
            let loc = self.loc_start_with(spos)
            go(Field(record=left, accessor~, loc~))
          }
          _ => left
        }
      }

      go(self.parse_simple_expr(parsed~))
    }
  }
}

///|
fn State::parse_constr_expr(
  self : Self,
  spos~ : Position,
  extra_info~ : @syntax.ConstructorExtraInfo,
) -> @syntax.Expr {
  if self.mode is Panic(loc~, ..) {
    return Hole(loc~, kind=Synthesized)
  }
  let name_loc_start = self.peek_spos()
  let constr_name = match extra_info {
    Package(_) => self.expect_dot_uident(context="constructor name")
    NoExtraInfo | TypeName(_) => self.expect_uident(context="constructor name")
  }
  let name_loc = self.loc_start_with(name_loc_start)
  let loc = self.loc_start_with(spos)
  let constr : @syntax.Constructor = {
    extra_info,
    name: { name: constr_name, loc: name_loc },
    loc: name_loc,
  }
  // constructor with arguments is handled by [Apply],
  // and desugared during ast linting
  Constr(constr~, loc~)
}

///|
priv enum IndexingSyntaxFragment {
  ArrayIndex(@syntax.Expr)
  SliceIndex(@syntax.Expr?, @syntax.Expr?)
}

///|
fn State::parse_argument(self : Self) -> @syntax.Argument {
  if self.mode is Panic(loc~, ..) {
    return { value: Hole(loc~, kind=Synthesized), kind: @syntax.Positional }
  }
  match self.peek_token() {
    // `label=expr` or `label?=expr`
    LIDENT(label_name) | UIDENT(label_name) if self.peek_token(nth=1) is EQUAL ||
      (self.peek_token(nth=1) is QUESTION && self.peek_token(nth=2) is EQUAL) => {
      let label : @syntax.Label = {
        name: label_name,
        loc: self.peek_location(),
      }
      self.skip() // luident
      let kind : @syntax.ArgumentKind = if self.peek_token() is QUESTION {
        let question_loc = self.peek_location()
        self.skip()
        LabelledOption(label~, question_loc~)
      } else {
        Labelled(label)
      }
      self.expect_token(TK_EQUAL) // `=`
      let value = self.parse_expr()
      { value, kind }
    }
    // label~
    POST_LABEL(label_name) => {
      let spos = self.peek_spos()
      self.skip()
      let label : @syntax.Label = {
        name: label_name,
        loc: self.peek_location(),
      }
      // error recovery for `label~=value`
      if self.peek_token() is EQUAL {
        self.skip()
        let value = self.parse_expr()
        let loc = self.loc_start_with(spos)
        self.report_error({
          loc,
          msg: "Invalid syntax `label~=value`. Do you mean `label=value`?",
        })
        { value, kind: Labelled(label) }
      } else {
        let value = @syntax.label_to_expr(
          loc=Location::trim_last_char(label.loc),
          label,
        )
        { value, kind: LabelledPun(label) }
      }
    }
    // label?
    LIDENT(label_name) if self.peek_token(nth=1) is QUESTION &&
      (self.peek_token(nth=2) is COMMA || self.peek_token(nth=2) is RPAREN) => {
      let label : @syntax.Label = {
        name: label_name,
        loc: self.peek_location(),
      }
      self.expect_token(TK_LIDENT) // lident
      let question_loc = self.peek_location()
      self.expect_token(TK_QUESTION) // `?`
      let kind = @syntax.LabelledOptionPun(label~, question_loc~)
      let value = @syntax.label_to_expr(loc=label.loc, label)
      { value, kind }
    }
    _ => {
      let value = self.parse_expr()
      { value, kind: Positional }
    }
  }
}

///|
fn State::parse_func(
  self : Self,
  has_error~ : Location?,
  is_async~ : Location?,
) -> @syntax.Func {
  if self.mode is Panic(loc~, ..) {
    return @syntax.Func::{
      parameters: @list.new(),
      params_loc: loc,
      body: @syntax.Hole(loc~, kind=Synthesized),
      return_type: None,
      error_type: @syntax.NoErrorType,
      kind: Lambda,
      has_error,
      is_async,
      loc,
    }
  }
  match self.peek_token() {
    LPAREN => {
      let param_loc_start = self.peek_spos()
      self.push_sync(TK_LBRACE)
      let params = self.surround_series(
        left=TK_LPAREN,
        right=TK_RPAREN,
        delim=TK_COMMA,
        fn(s) -> @syntax.Parameter { s.parse_parameter() },
      )
      let params_loc = self.loc_start_with(param_loc_start)
      let (return_type, error_type) = self.parse_func_return_type()
      self.pop_sync(TK_LBRACE)
      let block = self.parse_block_expr()
      let func_loc = @syntax.no_location
      @syntax.Func::{ // Will be set by caller
        parameters: @list.from_array(params),
        params_loc,
        body: block,
        return_type,
        error_type,
        kind: Lambda,
        has_error,
        is_async,
        loc: func_loc,
      }
    }
    other => {
      self.report_error({
        loc: self.peek_location(),
        msg: "Failed to parse func: expected '(' or '{', got \{other}",
      })
      let loc = self.panic()
      @syntax.Func::{
        parameters: @list.new(),
        params_loc: loc,
        body: @syntax.Hole(loc~, kind=Synthesized),
        return_type: None,
        error_type: @syntax.NoErrorType,
        kind: Lambda,
        has_error,
        is_async,
        loc,
      }
    }
  }
}

///|
fn State::parse_parameter(self : Self) -> @syntax.Parameter {
  if self.mode is Panic(loc~, ..) {
    return @syntax.Positional(binder={ name: "?binder", loc }, ty=None)
  }
  match self.peek_token() {
    UNDERSCORE => {
      let loc = self.peek_location()
      self.skip()
      let param_annot = self.parse_opt_annot()
      @syntax.DiscardPositional(ty=param_annot, loc~)
    }
    // `label~ : Type` 
    // or `label~ : Type = expr` (same as `label? : Type = expr`, deprecated syntax)
    POST_LABEL(label_name) => {
      let param_binder : @syntax.Binder = {
        name: label_name,
        loc: Location::trim_last_char(self.peek_location()),
      }
      self.skip() // skip POST_LABEL
      let param_annot = self.parse_opt_annot()
      match self.peek_token() {
        EQUAL if self.mode is Normal => {
          self.skip()
          let default = self.parse_expr()
          Optional(default~, binder=param_binder, ty=param_annot)
        }
        _ => Labelled(binder=param_binder, ty=param_annot)
      }
    }
    // `label? : Type` or `label? : Type = default`
    LIDENT(label_name) if self.peek_token(nth=1) is QUESTION => {
      let param_binder : @syntax.Binder = {
        name: label_name,
        loc: self.peek_location(),
      }
      self.skip() // skip LIDENT
      self.skip() // skip QUESTION
      let param_annot = self.parse_opt_annot()
      match self.peek_token() {
        // default value
        EQUAL if self.mode is Normal => {
          self.skip()
          let default = self.parse_expr()
          Optional(default~, binder=param_binder, ty=param_annot)
        }
        _ => QuestionOptional(binder=param_binder, ty=param_annot)
      }
    }
    // `param : Type`
    LIDENT(param_name) => {
      let param_binder : @syntax.Binder = {
        name: param_name,
        loc: self.peek_location(),
      }
      self.skip()
      let param_annot = self.parse_opt_annot()
      if self.peek_token() is EQUAL && self.mode is Normal {
        // error recovery for `param : Type = expr`
        let eq_loc = self.peek_location()
        self.skip()
        let default = self.parse_expr()
        self.report_error({
          loc: eq_loc,
          msg: "Only labeled argument can have default value",
        })
        Optional(default~, binder=param_binder, ty=param_annot)
      } else {
        Positional(binder=param_binder, ty=param_annot)
      }
    }
    other => {
      let other_loc = self.peek_location()
      self.report_failed_to_parse(other, "parameter name or label", other_loc)
      self.panic() |> ignore
      Positional(binder={ name: "?binder", loc: @syntax.no_location }, ty=None)
    }
  }
}

///|
fn State::parse_apply_attr(self : Self) -> @syntax.ApplyAttr {
  if self.mode is Panic(..) {
    return NoAttr
  }
  match self.peek_token() {
    EXCLAMATION => {
      self.skip()
      Exclamation
    }
    QUESTION => {
      self.skip()
      Question
    }
    _ => NoAttr
  }
}

///|
fn State::parse_simple_expr(
  self : Self,
  parsed~ : @syntax.Expr?,
) -> @syntax.Expr {
  if self.mode is Panic(loc~, ..) {
    return Hole(loc~, kind=Synthesized)
  }
  if parsed is Some(e) {
    return e
  }
  match self.peek_token() {
    TRUE => {
      let loc = self.peek_location()
      self.expect_token(TK_TRUE)
      Constant(c=Bool(true), loc~)
    }
    FALSE => {
      let loc = self.peek_location()
      self.expect_token(TK_FALSE)
      Constant(c=Bool(false), loc~)
    }
    BYTE(x) => {
      let loc = self.peek_location()
      self.expect_token(TK_BYTE)
      Constant(c=Byte(x), loc~)
    }
    CHAR(x) => {
      let loc = self.peek_location()
      self.expect_token(TK_CHAR)
      Constant(c=Char(x), loc~)
    }
    INT(x) => {
      let loc = self.peek_location()
      self.expect_token(TK_INT)
      Constant(c=@syntax.make_int(x), loc~)
    }
    FLOAT(x) => {
      let loc = self.peek_location()
      self.expect_token(TK_FLOAT)
      Constant(c=@syntax.make_float(x), loc~)
    }
    DOUBLE(x) => {
      let loc = self.peek_location()
      self.expect_token(TK_DOUBLE)
      Constant(c=@syntax.make_double(x), loc~)
    }
    STRING(x) => {
      let loc = self.peek_location()
      self.expect_token(TK_STRING)
      Constant(c=String(x), loc~)
    }
    BYTES(x) => {
      let loc = self.peek_location()
      self.expect_token(TK_BYTES)
      Constant(c=Bytes(x), loc~)
    }
    MULTILINE_STRING(_) | MULTILINE_INTERP(_) => {
      let spos = self.peek_spos()
      let elems : Array[@syntax.MultilineStringElem] = []
      while true {
        match self.peek_token() {
          MULTILINE_STRING(x) => {
            self.expect_token(TK_MULTILINE_STRING)
            elems.push(String(x))
          }
          MULTILINE_INTERP(xs) => {
            self.expect_token(TK_MULTILINE_INTERP)
            let converted : Array[@syntax.InterpElem] = xs.map(fn(elem) {
              match elem {
                InterpLit(repr~, loc~) => Literal(repr~, loc~)
                InterpSource(source) => Source(source)
              }
            })
            elems.push(Interp(@list.from_array(converted)))
          }
          _ => break
        }
      }
      let loc = self.loc_start_with(spos)
      MultilineString(elems=@list.from_array(elems), loc~)
    }
    INTERP(xs) => {
      let loc = self.peek_location()
      self.skip()
      @syntax.make_interp_expr(loc~, xs)
    }
    PACKAGE_NAME(pkg) =>
      match self.peek_token(nth=1) {
        // @pkg.Type::
        DOT_LIDENT(_) | DOT_UIDENT(_) if self.peek_token(nth=2) is COLONCOLON =>
          self.parse_coloncolon_expr()
        // @pkg.Constr
        DOT_UIDENT(_) => {
          let spos = self.peek_spos()
          self.skip()
          self.parse_constr_expr(spos~, extra_info=Package(pkg))
        }
        // @pkg.f
        _ => {
          let spos = self.peek_spos()
          let name = self.parse_qual_lident(context="variable name")
          let loc = self.loc_start_with(spos)
          Ident(id={ name, loc }, loc~)
        }
      }
    LIDENT(_) | UIDENT(_) if self.peek_token(nth=1) is COLONCOLON =>
      self.parse_coloncolon_expr()
    AMPER => self.parse_coloncolon_expr()
    LIDENT(_) => {
      let spos = self.peek_spos()
      let var_ = self.parse_var()
      let loc = self.loc_start_with(spos)
      Ident(id=var_, loc~)
    }
    // Cons(x,y)
    UIDENT(_) => {
      let spos = self.peek_spos()
      self.parse_constr_expr(spos~, extra_info=NoExtraInfo)
    }
    ASYNC => {
      let is_async = Some(self.peek_location())
      let spos = self.peek_spos()
      self.expect_token(TK_ASYNC) // async
      // TODO: improve error recovery for case like `async \n trait`
      self.with_syncs([TK_LPAREN, TK_LBRACE], self => self.expect_token(TK_FN)) // fn
      let has_error = self.parse_optional_bang()
      let func = self.parse_func(has_error~, is_async~)
      let loc = self.loc_start_with(spos)
      Function(func~, loc~)
    }
    FN => {
      let spos = self.peek_spos()
      self.expect_token(TK_FN)
      let has_error = self.parse_optional_bang()
      let func = self.parse_func(has_error~, is_async=None)
      let loc = self.loc_start_with(spos)
      Function(func~, loc~)
    }
    LPAREN => self.parse_tuple_or_constraint()
    LBRACKET => {
      let spos = self.peek_spos()
      let mut to_spread = false
      let elems = self.surround_series(
          left=TK_LBRACKET,
          right=TK_RBRACKET,
          delim=TK_COMMA,
          fn(state) -> @syntax.SpreadableElem {
            match state.peek_token() {
              DOTDOT => {
                to_spread = true
                let spread_spos = state.peek_spos()
                state.expect_token(TK_DOTDOT)
                let expr = state.parse_expr()
                let loc = state.loc_start_with(spread_spos)
                Spread(expr~, loc~)
              }
              _ => Regular(state.parse_expr())
            }
          },
        )
        |> @list.from_array
      let loc = self.loc_start_with(spos)
      if to_spread {
        ArraySpread(elems~, loc~)
      } else {
        let exprs = elems.map(elem => {
          match elem {
            Regular(e) => e
            Spread(_) => panic()
          }
        })
        Array(exprs~, loc~)
      }
    }
    LBRACE => {
      // to parse a block expr or a record expr ?
      // things got a little bit tricky here ...
      let spos = self.peek_spos()
      match self.peek_token(nth=1) {
        DOTDOT => self.parse_record_expr(type_name=None)
        LIDENT(_) =>
          match self.peek_token(nth=2) {
            // In cases `{ x: ... }` `{ x, }` `{ x; }` `{ x }` the result is a record expression.
            SEMI(_) if self.peek_token(nth=3) is RBRACE =>
              self.parse_semi_trailing_record()
            COLON | COMMA | RBRACE => self.parse_record_expr(type_name=None)
            _ => {
              let loc = self.loc_start_with(spos)
              let expr = self.parse_block_expr()
              Group(expr~, group=Brace, loc~)
            }
          }
        RBRACE => self.parse_map_expr()
        TRUE
        | FALSE
        | CHAR(_)
        | INT(_)
        | BYTE(_)
        | DOUBLE(_)
        | FLOAT(_)
        | MINUS
        | BYTES(_)
        | STRING(_) if self.peek_token(nth=2) is COLON => self.parse_map_expr()
        _ => {
          let loc = self.loc_start_with(spos)
          let expr = self.parse_block_expr()
          Group(expr~, group=Brace, loc~)
        }
      }
    }
    UNDERSCORE => {
      let loc = self.peek_location()
      self.expect_token(TK_UNDERSCORE)
      Hole(loc~, kind=Incomplete)
    }
    other => {
      let other_loc = self.peek_location()
      self.report_failed_to_parse(other, "simple expression", other_loc)
      let loc = self.panic()
      Hole(loc~, kind=Synthesized)
    }
  }
}

///|
fn State::parse_coloncolon_expr(self : Self) -> @syntax.Expr {
  if self.mode is Panic(loc~, ..) {
    return Hole(loc~, kind=Synthesized)
  }
  let spos = self.peek_spos()
  let type_name = self.parse_type_name()
  self.expect_token(TK_COLONCOLON)
  match self.peek_token() {
    LIDENT(_) => {
      // TypeName::method
      let method_name = self.parse_label(context="")
      let loc = self.loc_start_with(spos)
      Method(type_name~, method_name~, loc~)
    }
    UIDENT(_) => self.parse_constr_expr(spos~, extra_info=TypeName(type_name))
    LBRACE => self.parse_record_expr(type_name=Some(type_name))
    other => {
      self.report_failed_to_parse(
        other,
        "method name, constructor name, \"[\" or \"{\"",
        self.peek_location(),
      )
      let skipped_loc = self.panic()
      let loc : Location = { start: spos, end: skipped_loc.end }
      @syntax.Constr(
        constr={
          extra_info: TypeName(type_name),
          name: { name: "?constr", loc: @syntax.no_location },
          loc,
        },
        loc~,
      )
    }
  }
}

///|
fn State::parse_field_def(self : Self) -> @syntax.FieldDef {
  if self.mode is Panic(loc~, ..) {
    return {
      label: { name: "?", loc },
      expr: Hole(loc~, kind=Synthesized),
      is_pun: false,
      loc,
    }
  }
  let spos = self.peek_spos()
  let label = self.parse_label(context="struct field")
  let expr_opt = match self.peek_token() {
    EQUAL => {
      // Error production for `field = expr` in struct expression
      let loc = self.peek_location()
      self.report_error({
        loc,
        msg: "Invalid equal sign in struct expression, use `:` instead",
      })
      self.skip() // skip =
      Some(self.parse_expr())
    }
    COLON => {
      self.skip() // skip :
      Some(self.parse_expr())
    }
    _ => None
  }
  let loc = self.loc_start_with(spos)
  match expr_opt {
    Some(expr) => { label, expr, is_pun: false, loc }
    None => {
      let expr = @syntax.label_to_expr(loc=label.loc, label)
      { label, expr, is_pun: true, loc }
    }
  }
}

///|
fn State::parse_semi_trailing_record(self : Self) -> @syntax.Expr {
  if self.mode is Panic(loc~, ..) {
    return Hole(loc~, kind=Synthesized)
  }
  let spos = self.peek_spos()
  self.expect_token(TK_LBRACE)
  let field = self.parse_field_def()
  self.expect_token(TK_SEMI)
  self.expect_token(TK_RBRACE)
  let loc = self.loc_start_with(spos)
  Record(type_name=None, fields=@list.from_array([field]), trailing=Semi, loc~)
}

///|
fn State::parse_record_expr(
  self : Self,
  type_name~ : @syntax.TypeName?,
) -> @syntax.Expr {
  if self.mode is Panic(loc~, ..) {
    return Hole(loc~, kind=Synthesized)
  }
  let spos = self.peek_spos()
  self.expect_token(TK_LBRACE)
  let (spread_record, fields, trailing_comma) = if self.peek_token() is DOTDOT {
    self.skip()
    let spread_record = self.parse_expr()
    let (fields, trailing) = if self.peek_token() is COMMA {
      self.expect_token(TK_COMMA)
      let (fields, trailing) = self.series_with_follow(
        delim=TK_COMMA,
        follow_set=[TK_RBRACE],
        State::parse_field_def,
      )
      (@list.from_array(fields), trailing)
    } else {
      (@list.new(), false)
    }
    (Some(spread_record), fields, trailing)
  } else {
    let (fields, trailing_semi) = self.series_with_follow(
      delim=TK_COMMA,
      follow_set=[TK_RBRACE],
      State::parse_field_def,
    )
    (None, @list.from_array(fields), trailing_semi)
  }
  self.expect_token(TK_RBRACE)
  let loc = self.loc_start_with(spos)
  match spread_record {
    Some(record) => RecordUpdate(type_name~, record~, fields~, loc~)
    None => {
      let trailing : @syntax.TrailingMark = if trailing_comma {
        Comma
      } else {
        None
      }
      Record(type_name~, fields~, trailing~, loc~)
    }
  }
}

///|
fn State::parse_map_expr(self : Self) -> @syntax.Expr {
  if self.mode is Panic(loc~, ..) {
    return Hole(loc~, kind=Synthesized)
  }
  fn parse_elem(state : State) -> @syntax.MapExprElem? {
    let spos = state.peek_spos()
    fn parse_map_expr_elem(key : @syntax.Constant) -> @syntax.MapExprElem? {
      state.skip()
      let key_loc = state.loc_start_with(spos)
      state.expect_token(TK_COLON)
      let expr = state.parse_expr()
      let loc = state.loc_start_with(spos)
      Some({ key, expr, key_loc, loc })
    }

    match state.peek_token() {
      TRUE => parse_map_expr_elem(Bool(true))
      FALSE => parse_map_expr_elem(Bool(false))
      CHAR(x) => parse_map_expr_elem(Char(x))
      INT(x) => parse_map_expr_elem(@syntax.make_int(x))
      BYTE(x) => parse_map_expr_elem(Byte(x))
      FLOAT(x) => parse_map_expr_elem(@syntax.make_float(x))
      DOUBLE(x) => parse_map_expr_elem(@syntax.make_double(x))
      MINUS => {
        state.skip()
        match state.peek_token() {
          INT(x) => parse_map_expr_elem(@syntax.make_int("-" + x))
          FLOAT(x) => parse_map_expr_elem(@syntax.make_float("-" + x))
          DOUBLE(x) => parse_map_expr_elem(@syntax.make_double("-" + x))
          other => {
            let other_loc = state.peek_location()
            state.report_failed_to_parse(
              other, "`-Int` or `-Float` in key of map pattern", other_loc,
            )
            state.panic() |> ignore
            None
          }
        }
      }
      BYTES(x) => parse_map_expr_elem(Bytes(x))
      STRING(x) => parse_map_expr_elem(String(x))
      other => {
        let other_loc = state.peek_location()
        state.report_unexpected(other, first_map_pattern_key, other_loc)
        state.panic() |> ignore
        None
      }
    }
  }

  let spos = self.peek_spos()
  let elems = self.surround_series(
    left=TK_LBRACE,
    right=TK_RBRACE,
    delim=TK_COMMA,
    parse_elem,
  )
  let elems = elems.filter_map(elem => elem) // TODO: add catMaybes to core?
  let loc = self.loc_start_with(spos)
  Map(elems=@list.from_array(elems), loc~)
}

///|
fn State::parse_opt_annot(self : Self) -> @syntax.Type? {
  if self.mode is Panic(..) {
    return None
  }
  if self.peek_token() is COLON {
    self.skip()
    Some(self.parse_type())
  } else {
    None
  }
}

///|
fn State::parse_block_expr_with_local_types(
  self : Self,
) -> (@list.List[@syntax.LocalTypeDecl], @syntax.Expr) {
  if self.mode is Panic(loc~, ..) {
    return (@list.new(), Hole(loc~, kind=Synthesized))
  }
  let spos = self.peek_spos()
  let (local_types, stmts) = self.surround(left=TK_LBRACE, right=TK_RBRACE, fn(
    self,
  ) {
    let local_types = []
    // TODO: maybe use delimited_with_follow instead
    let mut allow_following_stmt = true
    while self.mode is Normal && self.peek_token() is (STRUCT | ENUM | TYPE) {
      self.push_sync(TK_SEMI)
      local_types.push(self.parse_local_type_decl())
      self.pop_sync(TK_SEMI)
      allow_following_stmt = if self.peek_token() is SEMI(_) {
        self.skip()
        true
      } else {
        false
      }
    }
    let stmts = if allow_following_stmt {
      self.series_with_follow(
        delim=TK_SEMI,
        follow_set=[TK_RBRACE],
        State::parse_stmt,
      ).0
    } else {
      []
    }
    (@list.from_array(local_types), @list.from_array(stmts))
  })
  let loc = self.loc_start_with(spos)
  let expr = Statement::compact_rev(stmts.rev(), loc~) // FIXME: remove rev
  (local_types, expr)
}

///|
fn State::parse_block_expr(self : Self) -> @syntax.Expr {
  if self.mode is Panic(loc~, ..) {
    return @syntax.Hole(loc~, kind=Synthesized)
  }
  let loc_start = self.peek_spos()
  let stmts = self.surround_series(
    left=TK_LBRACE,
    right=TK_RBRACE,
    delim=TK_SEMI,
    fn(s) -> Statement { s.parse_stmt() },
  )
  let loc = self.loc_start_with(loc_start)
  Statement::compact_rev(@list.from_array(stmts).rev(), loc~)
}

///|
fn State::parse_letand_func(self : Self) -> @syntax.Func {
  if self.mode is Panic(loc~, ..) {
    return @syntax.Func::{
      parameters: @list.new(),
      params_loc: loc,
      body: Hole(loc~, kind=Synthesized),
      return_type: None,
      error_type: NoErrorType,
      kind: Lambda,
      has_error: None,
      is_async: None,
      loc,
    }
  }
  fn parse_arrow_fn_param() -> @syntax.ArrowFnParam {
    if self.peek_token() is UNDERSCORE {
      let loc = self.peek_location()
      self.expect_token(TK_UNDERSCORE)
      Unnamed(loc)
    } else {
      let binder = self.parse_binder(context="function parameter")
      Named(binder)
    }
  }

  let spos = self.peek_spos()
  match self.peek_token() {
    LPAREN => {
      let parameters = self.surround_series(
        left=TK_LPAREN,
        right=TK_RPAREN,
        delim=TK_COMMA,
        fn(self) -> (@syntax.ArrowFnParam, @syntax.Type?) {
          let param = parse_arrow_fn_param()
          let ty_opt = self.parse_opt_annot()
          (param, ty_opt)
        },
      )
      let params_loc = self.loc_start_with(spos)
      self.expect_token(TK_FAT_ARROW)
      let body = self.parse_expr_stmt_no_break_continue_return()
      let loc = self.loc_start_with(spos)
      @syntax.make_arrow_fn(
        @list.from_array(parameters),
        body,
        params_loc~,
        loc~,
      )
    }
    LIDENT(_) | UNDERSCORE => {
      let param = parse_arrow_fn_param()
      let params_loc = self.loc_start_with(spos)
      self.expect_token(TK_FAT_ARROW)
      let body = self.parse_expr_stmt_no_break_continue_return()
      let loc = self.loc_start_with(spos)
      @syntax.make_arrow_fn(
        @list.from_array([(param, None)]),
        body,
        params_loc~,
        loc~,
      )
    }
    _ => {
      let is_async = if self.peek_token() is ASYNC {
        let loc = self.peek_location()
        self.expect_token(TK_ASYNC)
        Some(loc)
      } else {
        None
      }
      self.with_syncs([TK_LPAREN, TK_LBRACE], self => self.expect_token(TK_FN)) // fn
      let has_error = self.parse_optional_bang()
      self.parse_func(has_error~, is_async~)
    }
  }
}

///|
fn State::parse_stmt(self : Self) -> Statement {
  if self.mode is Panic(loc~, ..) {
    return Statement::StmtExpr(Hole(loc~, kind=Synthesized))
  }
  match self.peek_token() {
    LET => {
      let spos = self.peek_spos()
      self.expect_token(TK_LET)
      if self.peek_token() is MUTABLE {
        self.expect_token(TK_MUTABLE)
        let binder = self.parse_binder(context="variable name")
        let ty_opt = self.parse_opt_annot()
        self.expect_token(TK_EQUAL)
        let expr = self.parse_expr()
        let loc = self.loc_start_with(spos)
        StmtLetmut(binder~, ty_opt~, expr~, loc~)
      } else {
        let pat = self.parse_pattern()
        let ty_opt = self.parse_opt_annot()
        let pat : @syntax.Pattern = match ty_opt {
          Some(ty) => Constraint(pat~, ty~, loc=pat.loc().merge(ty.loc()))
          None => pat
        }
        self.expect_token(TK_EQUAL)
        let expr = self.parse_expr()
        let loc = self.loc_start_with(spos)
        StmtLet(pat~, expr~, loc~)
      }
    }
    LETREC => {
      let spos = self.peek_spos()
      self.expect_token(TK_LETREC)
      let bindings = self.sepby1(delim=TK_AND, fn(self) {
        let binder = self.parse_binder(context="variable name")
        let ty_opt = self.parse_opt_annot()
        self.expect_token(TK_EQUAL)
        let func = self.parse_letand_func()
        (binder, ty_opt, func)
      })
      let loc = self.loc_start_with(spos)
      StmtLetand(bindings=@list.from_array(bindings), loc~)
    }
    ASYNC if self.peek_token(nth=1) is FN && self.peek_token(nth=2) is LIDENT(_) => {
      let is_async = Some(self.peek_location())
      let spos = self.peek_spos()
      self.skip() // async
      self.expect_token(TK_FN)
      let binder = self.parse_binder(context="")
      let has_error = self.parse_optional_bang()
      let func = self.parse_func(has_error~, is_async~)
      let loc = self.loc_start_with(spos)
      StmtFunc(binder~, func~, loc~)
    }
    FN if self.peek_token(nth=1) is LIDENT(_) => {
      let spos = self.peek_spos()
      self.expect_token(TK_FN)
      let binder = self.parse_binder(context="")
      let has_error = self.parse_optional_bang()
      let func = self.parse_func(has_error~, is_async=None)
      let loc = self.loc_start_with(spos)
      StmtFunc(binder~, func~, loc~)
    }
    GUARD => {
      let spos = self.peek_spos()
      self.skip()
      let cond = self.with_syncs([TK_ELSE], State::try_parse_condition_expr)
      let otherwise = if self.peek_token() is ELSE {
        self.skip()
        Some(self.parse_block_expr())
      } else {
        None
      }
      let loc = self.loc_start_with(spos)
      StmtGuard(cond~, otherwise~, loc~)
    }
    DEFER => {
      let spos = self.peek_spos()
      self.skip()
      let expr = self.try_parse_pipe_expr()
      let loc = self.loc_start_with(spos)
      StmtDefer(expr~, loc~)
    }
    _ => StmtExpr(self.parse_expr_stmt())
  }
}

///|
fn State::handle_unexpected_expr_or_statement(
  self : Self,
  context~ : String,
) -> @syntax.Expr {
  let token = self.peek_token()
  fn handle_stmt(self : State) -> @syntax.Expr {
    let stmt = self.parse_stmt()
    // TODO: add stmt.loc()
    let loc = match stmt {
      StmtExpr(expr) => expr.loc()
      StmtLet(loc~, ..)
      | StmtLetmut(loc~, ..)
      | StmtFunc(loc~, ..)
      | StmtLetand(loc~, ..)
      | StmtDefer(loc~, ..)
      | StmtGuard(loc~, ..) => loc
    }
    Statement::compact_rev(@list.from_array([stmt]), loc~)
  }

  let (actual, expr) = match token {
    IF => ("if expression", self.parse_if_expr())
    MATCH => ("match expression", self.parse_match_expr())
    LOOP => ("loop expression", self.parse_loop_expr(label=None))
    WHILE => ("while expression", self.parse_while_loop(label=None))
    FOR => ("for expression", self.parse_for_expr(label=None))
    TRY => ("try expression", self.parse_try_expr())
    TRY_EXCLAMATION => ("`try!` expression", self.parse_try_exclamation())
    TRY_QUESTION => ("`try?` expression", self.parse_try_question())
    LET | LETREC => ("let statement", handle_stmt(self))
    GUARD => ("guard statement", handle_stmt(self))
    DEFER => ("defer statement", handle_stmt(self))
    BREAK => ("break statement", handle_stmt(self))
    CONTINUE => ("break statement", handle_stmt(self))
    RETURN => ("return statement", handle_stmt(self))
    _ => panic()
  }
  let loc = expr.loc()
  self.report_error({ loc, msg: "Unexpected \{actual} in \{context}" })
  expr
}

///|
fn State::try_parse_condition_expr(self : Self) -> @syntax.Expr {
  // an error production to handle unexpected expression in condition. 
  // For example: match if true {0} else {1} { ... => ... }
  match self.peek_token() {
    IF
    | MATCH
    | LOOP
    | WHILE
    | FOR
    | TRY
    | TRY_QUESTION
    | TRY_EXCLAMATION
    | LET
    | LETREC
    | GUARD
    | BREAK
    | RETURN
    | CONTINUE => self.handle_unexpected_expr_or_statement(context="condition")
    _ => self.parse_infix_expr(parsed=None)
  }
}

///|
fn State::parse_assign_or_simple_expr(self : Self) -> @syntax.Expr {
  let spos = self.peek_spos()
  let lhs = self.parse_expr()
  match self.peek_token() {
    EQUAL => {
      self.expect_token(TK_EQUAL)
      let rhs = self.parse_expr()
      let loc = self.loc_start_with(spos)
      match @syntax.make_assign_opt(loc~, lhs, rhs) {
        Some(res) => res
        None => {
          self.report_error({
            loc: lhs.loc(),
            msg: "Invalid left value in assignment",
          })
          lhs
        }
      }
    }
    AUGMENTED_ASSIGNMENT(op) => {
      let op_loc = self.peek_location()
      let op : @syntax.Var = { name: Ident(name=op), loc: op_loc }
      self.expect_token(TK_AUGMENTED_ASSIGNMENT)
      let rhs = self.parse_expr()
      let loc = self.loc_start_with(spos)
      match @syntax.make_augmented_assign_opt(loc~, op, lhs, rhs) {
        Some(res) => res
        None => {
          self.report_error({
            loc: lhs.loc(),
            msg: "Invalid left value in assignment",
          })
          lhs
        }
      }
    }
    _ => lhs
  }
}

///|
fn State::parse_expr_stmt_no_break_continue_return(self : Self) -> @syntax.Expr {
  if self.mode is Panic(loc~, ..) {
    return Hole(loc~, kind=Synthesized)
  }
  match self.peek_token() {
    RAISE => {
      let loc_start = self.peek_spos()
      self.expect_token(TK_RAISE)
      let expr = self.parse_expr()
      let loc_ = self.loc_start_with(loc_start)
      @syntax.Raise(err_value=expr, loc=loc_)
    }
    ELLIPSIS => {
      let loc_ = self.peek_location()
      self.skip()
      @syntax.Hole(kind=Todo, loc=loc_)
    }
    _ => self.parse_assign_or_simple_expr()
  }
}

///|
fn State::parse_expr_stmt(self : Self) -> @syntax.Expr {
  match self.peek_token() {
    BREAK => {
      let spos = self.peek_spos()
      self.expect_token(TK_BREAK)
      let label = if self.peek_token() is POST_LABEL(label_name) {
        self.skip()
        Some(@syntax.Label::{ name: label_name, loc: self.peek_location() })
      } else {
        None
      }
      let arg = self.optional(first_expr, State::parse_expr)
      Break(arg~, loc=self.loc_start_with(spos), label~)
    }
    CONTINUE => {
      let spos = self.peek_spos()
      self.expect_token(TK_CONTINUE)
      let label : @syntax.Label? = match self.peek_token() {
        //  Consider the case:
        //  ```
        //  continue l~: loop 0 { ... } 
        // ```
        //  Here the label l~ is for the loop instead of the continue statement.
        POST_LABEL(label_name) if !(self.peek_token(nth=1) is COLON) => {
          self.skip()
          Some({ name: label_name, loc: self.peek_location() })
        }
        _ => None
      }
      let args = self.sepby(
          delim=TK_COMMA,
          first_set=first_expr,
          State::parse_expr,
        )
        |> @list.from_array
      Continue(args~, loc=self.loc_start_with(spos), label~)
    }
    RETURN => {
      let spos = self.peek_spos()
      self.expect_token(TK_RETURN)
      let expr = self.optional(first_expr, State::parse_expr)
      let loc_ = self.loc_start_with(spos)
      Return(return_value=expr, loc=loc_)
    }
    _ => self.parse_expr_stmt_no_break_continue_return()
  }
}

///|
fn State::parse_error_type(self : Self) -> @syntax.Type {
  if self.mode is Panic(loc~, ..) {
    return Any(loc~)
  }
  let spos = self.peek_spos()
  match self.peek_token() {
    UNDERSCORE => {
      self.skip()
      let loc = self.loc_start_with(spos)
      Any(loc~)
    }
    UIDENT(id) if self.peek_token(nth=1) is QUESTION => {
      self.skip() // UIDENT
      self.skip() // QUESTION  
      let name = @syntax.LongIdent::Ident(name=id + "?")
      let loc = self.loc_start_with(spos)
      let constr_id : @syntax.ConstrId = { id: name, loc }
      Name(constr_id~, tys=@list.new(), loc~)
    }
    _ => {
      let name = self.parse_qual_luident()
      let loc = self.loc_start_with(spos)
      let constr_id : @syntax.ConstrId = { id: name, loc }
      Name(constr_id~, tys=@list.new(), loc~)
    }
  }
}

///|
fn State::parse_error_annotation(self : Self) -> @syntax.ErrorType {
  if self.mode is Panic(..) {
    return NoErrorType
  }
  match self.peek_token() {
    RAISE => {
      let raise_loc = self.peek_location()
      self.skip()
      match self.peek_token() {
        QUESTION => {
          let question_loc = self.peek_location()
          self.skip()
          let loc = raise_loc.merge(question_loc)
          let fake_error = @syntax.Type::Name(
            constr_id={ id: Ident(name="Error"), loc },
            tys=@list.new(),
            loc~,
          )
          MaybeError(ty=fake_error)
        }
        PACKAGE_NAME(_) | UIDENT(_) | LIDENT(_) | UNDERSCORE => {
          let ty = self.parse_error_type()
          ErrorType(ty~)
        }
        _ => DefaultErrorType(loc=raise_loc)
      }
    }
    NORAISE => {
      let loc = self.peek_location()
      self.skip()
      Noraise(loc~)
    }
    _ => NoErrorType
  }
}

///|
fn State::parse_return_type(self : Self) -> (@syntax.Type, @syntax.ErrorType) {
  if self.mode is Panic(loc~, ..) {
    return (Any(loc~), NoErrorType)
  }
  fn report_missing_return_type(token) -> (@syntax.Type, @syntax.ErrorType) {
    let loc = self.peek_location()
    State::report_failed_to_parse(self, token, "return type annotation", loc)
    (Any(loc~), NoErrorType)
  }

  match self.peek_token() {
    // Error production rules for missing return type
    LBRACE | EQUAL | EOF => report_missing_return_type(self.peek_token())
    // Error production for first_impl tokens (approximate from pattern observation)
    token if first_impl_.contains(token.kind()) && !(token is ASYNC) =>
      report_missing_return_type(token)
    _ => {
      let (ty_res_is_simple, ty_res) = self.parse_type_aux()
      match self.peek_token() {
        _ if !ty_res_is_simple => (ty_res, NoErrorType)
        EXCLAMATION => {
          let loc = self.peek_location()
          self.expect_token(TK_EXCLAMATION)
          match self.peek_token() {
            PACKAGE_NAME(_) | UIDENT(_) | LIDENT(_) | UNDERSCORE => {
              let ty = self.parse_error_type()
              (ty_res, ErrorType(ty~))
            }
            _ => (ty_res, DefaultErrorType(loc~))
          }
        }
        QUESTION => {
          self.skip()
          let ty = self.parse_error_type()
          (ty_res, MaybeError(ty~))
        }
        _ => {
          let err = self.parse_error_annotation()
          (ty_res, err)
        }
      }
    }
  }
}

///|
fn State::parse_func_return_type(
  self : Self,
) -> (@syntax.Type?, @syntax.ErrorType) {
  if self.mode is Panic(..) {
    return (None, NoErrorType)
  }
  if self.peek_token() is THIN_ARROW {
    self.skip()
    let (ret_ty, err_ty) = self.parse_return_type()
    (Some(ret_ty), err_ty)
  } else {
    let err = self.parse_error_annotation()
    (None, err)
  }
}

///|
fn State::parse_type(self : Self) -> @syntax.Type {
  let (_, ty) = self.parse_type_aux()
  ty
}

///|
/// The bool indicates whether the type is a simple type.
fn State::parse_type_aux(self : Self) -> (Bool, @syntax.Type) {
  if self.mode is Panic(loc~, ..) {
    return (false, @syntax.Type::Any(loc~))
  }
  fn parse_question_mark(spos, ty) -> (Bool, @syntax.Type) {
    if self.peek_token() is QUESTION &&
      !first_type_.contains(self.peek_token(nth=1).kind()) {
      let constr_loc = self.peek_location()
      self.expect_token(TK_QUESTION)
      let loc = self.loc_start_with(spos)
      let ty = @syntax.make_option_type(loc~, constr_loc~, ty)
      parse_question_mark(spos, ty)
    } else {
      (true, ty)
    }
  }

  let spos = self.peek_spos()
  match self.peek_token() {
    PACKAGE_NAME(_) | LIDENT(_) | UIDENT(_) => {
      let name_loc = self.peek_location()
      let name = self.parse_qual_luident()
      let args = self.optional([TK_LBRACKET], fn(self) {
        self.surround_series(
          left=TK_LBRACKET,
          right=TK_RBRACKET,
          delim=TK_COMMA,
          State::parse_type,
        )
        |> @list.from_array
      })
      let loc = self.loc_start_with(spos)
      let ty = @syntax.Type::Name(
        constr_id={ id: name, loc: name_loc },
        tys=args.unwrap_or(@list.new()),
        loc~,
      )
      parse_question_mark(spos, ty)
    }
    ASYNC => {
      let is_async = Some(self.peek_location())
      self.skip()
      let tys = self.surround_series(
          left=TK_LPAREN,
          right=TK_RPAREN,
          delim=TK_COMMA,
          State::parse_type,
        )
        |> @list.from_array
      self.expect_token(TK_THIN_ARROW)
      let (ty_res, ty_err) = self.parse_return_type()
      let loc = self.loc_start_with(spos)
      (false, Arrow(args=tys, res=ty_res, err=ty_err, is_async~, loc~))
    }
    LPAREN => {
      let tys = self.surround_series(
          left=TK_LPAREN,
          right=TK_RPAREN,
          delim=TK_COMMA,
          State::parse_type,
        )
        |> @list.from_array
      if tys is Empty {
        self.expect_token(TK_THIN_ARROW)
        let (ty_res, ty_err) = self.parse_return_type()
        let loc = self.loc_start_with(spos)
        (
          false,
          Arrow(args=@list.new(), res=ty_res, err=ty_err, is_async=None, loc~),
        )
      } else {
        match self.peek_token() {
          THIN_ARROW => {
            self.expect_token(TK_THIN_ARROW)
            let (ty_res, ty_err) = self.parse_return_type()
            let loc = self.loc_start_with(spos)
            (
              false,
              Arrow(args=tys, res=ty_res, err=ty_err, is_async=None, loc~),
            )
          }
          _ => {
            let tys_array = tys.to_array()
            match tys_array {
              [] => panic()
              [ty] => parse_question_mark(spos, ty)
              _ => {
                let loc = self.loc_start_with(spos)
                let ty : @syntax.Type = Tuple(tys~, loc~)
                parse_question_mark(spos, ty)
              }
            }
          }
        }
      }
    }
    UNDERSCORE => {
      let loc = self.peek_location()
      self.expect_token(TK_UNDERSCORE)
      let ty : @syntax.Type = Any(loc~)
      parse_question_mark(spos, ty)
    }
    AMPER => {
      self.skip()
      let spos = self.peek_spos()
      let lid = self.parse_qual_luident()
      let loc = self.loc_start_with(spos)
      let ty = @syntax.Type::Object({ id: lid, loc })
      parse_question_mark(spos, ty)
    }
    other => {
      let other_loc = self.peek_location()
      self.report_failed_to_parse(other, "type", other_loc)
      let loc = self.panic()
      (false, @syntax.Type::Any(loc~))
    }
  }
}

///|
fn State::parse_opt_typ_params(
  self : Self,
) -> @list.List[@syntax.TypeVarBinder] {
  if self.mode is Panic(..) {
    return @list.new()
  }
  self
  .optional([TK_LBRACKET], fn(state) {
    state.surround_series(
      left=TK_LBRACKET,
      right=TK_RBRACKET,
      delim=TK_COMMA,
      State::parse_tvar_binder,
    )
  })
  .unwrap_or([])
  |> @list.from_array
}

///|
pub fn parse(tokens : Array[Triple]) -> (Impls, Array[Report]) {
  let self : State = {
    tokens,
    diagnostics: [],
    next: 0,
    syncs: [],
    mode: Normal,
    parsed_position: dummy_pos,
  }
  let impls = self.parse_toplevel()
  (impls, self.diagnostics)
}

///|
pub fn parse_expr(tokens : Array[Triple]) -> (@syntax.Expr, Array[Report]) {
  let self : State = {
    tokens,
    diagnostics: [],
    next: 0,
    syncs: [],
    mode: Normal,
    parsed_position: dummy_pos,
  }
  let expr = self.parse_expr()
  (expr, self.diagnostics)
}
