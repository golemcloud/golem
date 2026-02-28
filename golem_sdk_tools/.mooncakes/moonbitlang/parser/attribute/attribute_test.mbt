///|
let dummy_loc : @basic.Location = {
  let pos = @basic.Position::{ fname: "", lnum: 0, bol: 0, cnum: 0 }
  { start: pos, end: pos }
}

///|
test "attribute" {
  let attr = Attribute::new(loc=dummy_loc, ("qual", Some("name"), "(a,b,c)"))
  inspect(
    attr,
    content=(
      #|{loc: -0:1-0:1, raw: "#qual.name", parsed: Some(Apply(qual.name, @list.from_array([Expr(Ident(a)), Expr(Ident(b)), Expr(Ident(c))])))}
    ),
  )
  let attr = Attribute::new(
    loc=dummy_loc,
    ("qual", Some("name"), "(label1=abc, label2=cde)"),
  )
  inspect(
    attr,
    content=(
      #|{loc: -0:1-0:1, raw: "#qual.name", parsed: Some(Apply(qual.name, @list.from_array([Labeled("label1", Ident(abc)), Labeled("label2", Ident(cde))])))}
    ),
  )
  let attr = Attribute::new(
    loc=dummy_loc,
    ("qual", Some("name"), "(f(false), g(true))"),
  )
  inspect(
    attr,
    content=(
      #|{loc: -0:1-0:1, raw: "#qual.name", parsed: Some(Apply(qual.name, @list.from_array([Expr(Apply(f, @list.from_array([Expr(Bool(false))]))), Expr(Apply(g, @list.from_array([Expr(Bool(true))])))])))}
    ),
  )
  let attr = Attribute::new(
    loc=dummy_loc,
    ("qual", Some("name"), "(\"abcdef\", g(true))"),
  )
  inspect(
    attr,
    content=(
      #|{loc: -0:1-0:1, raw: "#qual.name", parsed: Some(Apply(qual.name, @list.from_array([Expr(String("abcdef")), Expr(Apply(g, @list.from_array([Expr(Bool(true))])))])))}
    ),
  )
  let attr = Attribute::new(loc=dummy_loc, ("qual", Some("name"), ""))
  inspect(
    attr,
    content=(
      #|{loc: -0:1-0:1, raw: "#qual.name", parsed: Some(Ident(qual.name))}
    ),
  )
}
