
fn all_true(list: @list.List[Bool]) -> Bool {
  match list {
    @list.List::Empty => true
    @list.List::More(hd, tail=tl) => hd && all_true(tl)
  }
}

fn all_true2(list: @list.List[Bool]) -> Bool {
  match list {
    @list.List::Empty => true
    @list.List::More(hd, tail=tl) => if hd { all_true2(tl) } else { false }
  }
}


fn init {
  println(all_true(@list.cons(true, @list.empty())))
  println(all_true2(@list.cons(true, @list.empty())))
}


fn any_true(list: @list.List[Bool]) -> Bool {
  match list {
    @list.List::Empty => false
    @list.List::More(hd, tail=tl) => hd || any_true(tl)
  }
}

fn any_true2(list: @list.List[Bool]) -> Bool {
  match list {
    @list.List::Empty => false
    @list.List::More(hd, tail=tl) => if hd { true } else { any_true2(tl) }
  }
}


fn init {
  println(any_true(@list.cons(true, @list.empty())))
  println(any_true2(@list.cons(true, @list.empty())))
}
