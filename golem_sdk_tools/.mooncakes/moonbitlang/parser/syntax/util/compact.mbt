///|
/// The `Statement` type is not part of MoonBit's AST.
/// In fact, a statement in MoonBit is simply an incomplete expression. For example, consider a
/// block with a let statement:
///
/// ```skip
/// {
///   let _ = a
/// }
/// ```
///
/// The resulting AST is `Expr::Let(pattern=Any, expr=Ident("a"), body=Unit)`,
/// where the result value is the body. Since the body part is missing, we fill it with a `Unit`.
/// 
/// This type provides methods to convert an `Array[Statement]` into an `Expr`.
pub(all) enum Statement {
  /// Single expression
  StmtExpr(Expr)
  /// let statement
  /// 
  /// ```skip
  /// let pat = expr
  /// ```
  StmtLet(pat~ : Pattern, expr~ : Expr, loc~ : Location)
  /// guard statement
  ///
  /// ```skip
  /// guard cond else { otherwise }
  /// ```
  StmtGuard(cond~ : Expr, otherwise~ : Expr?, loc~ : Location)
  /// let mut statement
  ///
  /// ```skip
  /// let mut binder : ty_opt = expr
  /// ```
  StmtLetmut(binder~ : Binder, ty_opt~ : Type?, expr~ : Expr, loc~ : Location)
  /// local function
  /// 
  /// ```skip
  /// fn local() { ... }
  /// ```
  StmtFunc(binder~ : Binder, func~ : Func, loc~ : Location)
  /// letand statement
  StmtLetand(bindings~ : @list.List[(Binder, Type?, Func)], loc~ : Location)
  /// defer statement
  StmtDefer(expr~ : Expr, loc~ : Location)
}

///|
pub fn Statement::compact_rev(
  ls : @list.List[Statement],
  loc~ : Location,
) -> Expr {
  match ls {
    More(StmtExpr(expr), tail=rest) => Statement::collect_rev(rest, expr)
    rest => {
      let loc = match rest {
        Empty => loc
        More(
          StmtLet(loc~, ..)
          | StmtLetmut(loc~, ..)
          | StmtFunc(loc~, ..)
          | StmtLetand(loc~, ..)
          | StmtDefer(loc~, ..)
          | StmtGuard(loc~, ..),
          ..
        ) => loc
        More(StmtExpr(_), ..) => panic()
      }
      Statement::collect_rev(rest, Expr::Unit(loc~, faked=true))
    }
  }
}

///|
pub fn Statement::collect_rev(
  rest : @list.List[Statement],
  cont : Expr,
) -> Expr {
  match rest {
    Empty => cont
    More(x, tail=xs) =>
      match x {
        StmtFunc(binder~, func~, loc~) =>
          Statement::collect_letrec(
            loc~,
            @list.singleton((binder, func)),
            xs,
            cont,
          )
        StmtExpr(_) =>
          Statement::collect_sequence(
            loc=loc_of_expression(cont),
            @list.empty(),
            rest,
            cont,
          )
        _ => {
          let acc = match x {
            StmtLet(pat~, expr~, loc~) => {
              let loc = loc.merge(cont.loc())
              Expr::Let(pattern=pat, expr~, loc~, body=cont)
            }
            StmtGuard(cond~, otherwise~, loc~) => {
              let loc = loc.merge(cont.loc())
              Expr::Guard(cond~, otherwise~, loc~, body=cont)
            }
            StmtLetmut(binder~, ty_opt~, expr~, loc~) => {
              let loc = loc.merge(cont.loc())
              Expr::LetMut(binder~, ty=ty_opt, expr~, loc~, body=cont)
            }
            StmtLetand(bindings~, loc~) => {
              let loc = loc.merge(loc_of_expression(cont))
              Expr::LetAnd(bindings~, loc~, body=cont)
            }
            StmtDefer(expr~, loc~) => {
              let loc = loc.merge(loc_of_expression(cont))
              Expr::Defer(expr~, loc~, body=cont)
            }
            StmtExpr(_) | StmtFunc(..) => panic()
          }
          Statement::collect_rev(xs, acc)
        }
      }
  }
}

///|
fn Statement::collect_sequence(
  loc~ : Location,
  acc : @list.List[Expr],
  todo : @list.List[Statement],
  cont : Expr,
) -> Expr {
  match todo {
    More(StmtExpr(expr), tail=rest) => {
      let loc = loc.merge(expr.loc())
      Statement::collect_sequence(loc~, acc.add(expr), rest, cont)
    }
    _ =>
      Statement::collect_rev(
        todo,
        Expr::Sequence(exprs=acc, last_expr=cont, loc~),
      )
  }
}

///|
pub fn Statement::collect_letrec(
  loc~ : Location,
  acc : @list.List[(Binder, Func)],
  todo : @list.List[Statement],
  cont : Expr,
) -> Expr {
  match todo {
    More(StmtFunc(binder~, func~, loc~), tail=rest) =>
      Statement::collect_letrec(loc~, acc.add((binder, func)), rest, cont)
    _ => {
      let expr = match acc {
        More((name, func), tail=Empty) =>
          Expr::LetFn(
            name~,
            func~,
            loc=Location::merge(loc, loc_of_expression(cont)),
            body=cont,
          )
        _ =>
          Expr::LetRec(
            bindings=acc,
            body=cont,
            loc=Location::merge(loc, loc_of_expression(cont)),
          )
      }
      Statement::collect_rev(todo, expr)
    }
  }
}
