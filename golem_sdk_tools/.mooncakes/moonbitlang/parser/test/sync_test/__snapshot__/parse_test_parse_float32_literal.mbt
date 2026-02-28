fn main {
  let _ = 3.14F
  let _ = 3.14e2F
  let f = 0xCDEF.CDEFp1F
  match f {
    3.14F | 3.14e2F | 0xCDEF.CDEFp1F => ... 
    _ => ...
  }
}
