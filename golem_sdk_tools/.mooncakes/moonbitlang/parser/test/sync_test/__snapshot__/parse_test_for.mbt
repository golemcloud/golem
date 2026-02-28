fn init {
  for ; ; { }
  // nested
  for i = 0; i < x; i = i + 1 {
    for j = i; j < y; j = j + 1 {
      f(i, j)
    }
  }
  // multiple binders
  for i = 0, j = 0; i + j < 100 && i * j < 100; i = i + j, j = j + i { }
  // no initializer
  for ; i + j < 100 || f(i, j) && g(i) || h(j); i = i + j, j = j + i { }
  // no conditionals
  for i = 0, j = i; ; i = i + j { }
  // no continue block
  for i = 0, j = 100; i + j < 10; { }
  // application
  f(for i = 0; ; i = i + 1 { i }, 3)
  // pipe
  (for ; ; {  }) |> f
}

fn errors() {
  // lhs of initializater must be binder
  for x.a = 0; ; { }
  for (i, j) = (0, 0); ; {  }
  // continuation must be assignment to binder
  for ; ; f() {  }
  for i = 0; ; i = i + 1, a[i] = i {  }
  // missing semi
  for i = 0 {  }
  for i = 0; i < 10 {  }
  // extra semi
  for i = 0; i < 1; i= i + 1; i = i + 1 {  }
  for i = 0; i < 1; i= i + 1; i = i + 1; i = i + 1 {  }
}

fn auto_semi() {
  for i = 0
      i < 10
      i = i + 1 {
    f(i)
  }

  for i = f(x), j = g(y)
      i + j < 10 || j + i < 10
      i = j, j = i {
    f(i, j)
  }
}

fn auto_semi_errors() {
  for i = 0
      i = i + 1 {
    f(i)
  }

  for i = f(x), j = g(y)
      i + j < 10 && j + i < 10
      i = j, j = i
      j = i, i = j {
    f(i, j)
  }
}

fn relaxed_for_with_else_part() {
  for ;; {
    ignore(5)
  } else {
    ignore(6)
  }

  for i = 0 {
    ignore(i)
  } else {
    ignore(i)
  }
}
