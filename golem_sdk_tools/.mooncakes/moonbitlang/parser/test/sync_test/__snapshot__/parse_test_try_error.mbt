fn init {
  // try in try
  try try 1 catch { _ => 2 } catch { _ => 3 }
}
fn init {
  // if in try
  try if true { 1 } else { 2 } catch { _ => 3 }
}

fn init {
  // loop in try
  try loop 1 { _ => 2 } catch { _ => 3 }
}

fn init {
  // match in try
  try match 1 { _ => 2 } catch { _ => 3 }
}

fn init {
  // try in try?
  try? try 1 catch { _ => 2 }
}

fn init {
  try 42 catch { _ => 2 } catch { _ => 3 }
}

fn init {
  // infix is tighter
  let _ = 1 catch { _ => 2 } + 3 catch { _ => 4}
}

fn init {
  // postfix is tighter
  let _ = 1 catch { _ => 2} as &Show
}