fn main {
  let _ = try try? 1 catch { _ => 0 }
  let _ = try try! 1 catch { _ => 0 }
  let _ = try let x = 1 catch { _ => 0 }
  let _ = try letrec f = () => 0 catch { _ => 0 }
  let _ = try guard true else { 0 } catch { _ => 0 }
  let _ = try break catch { _ => 0 }
  let _ = try return catch { _ => 0 }
  let _ = try continue catch { _ => 0 }
}

