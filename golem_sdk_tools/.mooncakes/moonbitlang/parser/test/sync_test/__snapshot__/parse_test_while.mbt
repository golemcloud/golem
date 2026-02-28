
fn init {
  while e {
    body()
  }

  let mut i = 0
  while i < len, i = i + 1 {
    body()
  }

  let mut i = 0
  let mut j = 0
  while i < len1 && j < len2, { i = i + 1; j = j + 1 } {
    body()
  }

  while parse error {
    body()
  }

  while parse error, { continue_block() } {
    body()
  }

  // must error, otherwise ambiguous (return nothing v.s. return empty record)
  while true, return {}
}
