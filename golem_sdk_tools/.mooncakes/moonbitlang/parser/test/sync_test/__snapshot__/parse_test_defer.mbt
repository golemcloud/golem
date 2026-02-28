test {
  defer x
  defer simple()
  defer socket.close()
  defer conn..send_farewell().close()
  defer x |> f
  defer {
    block()
    block()
  }
}
