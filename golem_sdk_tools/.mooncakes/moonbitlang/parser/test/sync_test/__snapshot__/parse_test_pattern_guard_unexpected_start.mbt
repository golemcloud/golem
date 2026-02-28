fn main {
  match 0 {
    _ if try? 1 => ()
    _ if try! 1 => ()
    _ if let x = 1 => ()
    _ if letrec f = () => 0 => ()
    _ if guard true else { () } => ()
    _ if break => ()
    _ if return => ()
    _ if continue => ()
    _ if loop 1 { _ => 0 } => ()
    _ if while true { } => ()
    _ if for ; ; { } => ()
    _ if try 1 catch { _ => 0 } => ()
  }
}
