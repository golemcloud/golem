fn f() -> Unit {
  try {
    h()
    g()
    raise "xxx"
  } catch {
    _ => panic()
  }
}

fn f() -> Unit {
  try { 1 + 2 } catch { _ => panic() }
}

fn f() -> Unit {
  try {
    try {
      1 + 2
    } catch {
      _ => {
        try { f(x) } catch { _ => panic() }
      }
    }
  } catch {
    _ => panic()
  }
}

fn f() -> Unit {
  try 1 + 2 catch { _ => panic() }
}

fn f() -> Unit {
  try f(x, y) catch { _ => panic() }
}

fn f() -> Unit {
  try 1 + 2 catch { _ => panic() }
}

fn f() -> Unit {
  try f(x, y) catch { _ => panic() }
}

fn f() -> Unit {
  try {1 + 2} catch { _ => panic() }
}

fn f() -> Unit {
  try {f(x, y)} catch { _ => panic() }
}

fn f() -> Unit {
  try (try 1 catch { _ => 2 }) catch { _ => try 3 catch { _ => 4 } }
}
