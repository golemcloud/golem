fn f() -> Unit {
  (try {
    x |> Ok
  } catch {
    _ => Err
  })
}
