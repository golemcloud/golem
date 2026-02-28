test "try catch syntax error" {
  fail("msg") catch {
    return
  }

  try fail("msg") catch {
    _ => ...
  } noraise {
    return
  }
}

test "loop syntax error" {
  loop 1 {
    let x = 10
    f()
  }
}

test "match syntax error" {
  match 1 {
    return
  }
}

fn f() -> Unit raise {
  fail("")
}

test "expecting pattern match case but encountered statement" {
  f() catch { if true { 1 } else { 0 } }
  f() catch { match 1 { ... } }
  f() catch { loop 1 { ... } }
  f() catch { while true { ... } }
  f() catch { for {...} }
  f() catch { try 1 catch { ... } }
  f() catch { try? f() }
  f() catch { try! f() }
  f() catch { let a = 1 }
  f() catch {
    letrec f = () => 1 
    and g = () => 2
  }
  f() catch { guard true }
  f() catch { break }
  f() catch { return }
  f() catch { continue }
  f() catch { raise Error }
  f() catch {
    fn local(){

    }
  }
  f() catch {
    async fn local() {

    }
  }
  f() catch {
    label~: for {...}
  }
  f() catch { mutable = 1 }
  f() catch { @pkg.binder = 1 }
}

test "... is valid in cases clause" {
  f() catch {
    ...
  }
}