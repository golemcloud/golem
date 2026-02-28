fn init {
  let _ = try a |> f |> g catch { _ => 2 }
  let _ = a |> f |> g catch { _ => 2 }
  let _ = try? a |> f |> g

  let _ = try a + b catch { _ => 2 }
  let _ = a + b catch { _ => 2 }
  let _ = try? a + b
  let _ = try 42 as &Show catch { _ => 2 }
  let _ = 42 as &Show catch { _ => 2 }
  let _ = try? 42 as &Show

  let _ = try 1 is 2 catch { _ => 2 }
  let _ = 1 is 2 catch { _ => 2 }
  let _ = try? 1 is 2

  let _ = try { x: 1 } catch { _ => 2}
  let _ = { x: 1 } catch { _ => 2}
  let _ = try? { x: 1 }

  let _ = try T::A catch { _ => 2 }
  let _ = T::A catch { _ => 2}
  let _ = try? T::A

  let _ = try { println(42) } catch { _ => 2 }
  let _ = { println(42) } catch { _ => 2 }
  let _ = try? { println(42) }

  let _ = try (_ => 42) catch { _ => 2 }
  let _ = (_ => 42) catch { _ => 2 }
  let _ = try? (_ => 42)
  
}

fn init {
  // infix is tighter
  let _ = (1 catch { _ => 2 }) + (3 catch { _ => 4})
  // postfix is tighter
  let _ = (1 catch { _ => 2}) as &Show
  // pipe is tighter
  let _ = (1 catch { _ => 2}) |> f catch { _ => 3}
  let _ = (1 catch { _ => 2}) |> (f catch { _ => 3})
  // let _ = match 1 catch { _ => 2 } {
  //   _ => 3
  // }

  // let _ = if 1 catch { _ => 2 } { println(42) }

}
