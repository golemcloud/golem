fn init {
  let _ = a[:]
  let _ = a[1:]
  let _ = a[:1]
  let _ = a[1:2]
  let _ = a[x:y]
  let _ = a[x+y:a.length()]
  let _ = a[x+y:a.length()][:]
  let _ = a[x+y:a.length()][1:a.length()]
}

test {
  a[0:1] |> f
}
