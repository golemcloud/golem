///|
test {
  let tuple = (1, 'a')
  inspect(pretty(tuple), content="(1, a)")
  inspect(
    render(tuple, width=3),
    content=(
      #|(
      #|  1,
      #|  a
      #|)
    ),
  )
  let tuple = (1, 'a', false)
  inspect(pretty(tuple), content="(1, a, false)")
  inspect(
    render(tuple, width=3),
    content=(
      #|(
      #|  1,
      #|  a,
      #|  false
      #|)
    ),
  )
  let tuple = (1, 'a', false, 1.5)
  inspect(pretty(tuple), content="(1, a, false, 1.5)")
  inspect(
    render(tuple, width=3),
    content=(
      #|(
      #|  1,
      #|  a,
      #|  false,
      #|  1.5
      #|)
    ),
  )
  let tuple = (1, 'a', false, 1.5, Some(1))
  inspect(pretty(tuple), content="(1, a, false, 1.5, Some(1))")
  inspect(
    render(tuple, width=10),
    content=(
      #|(
      #|  1,
      #|  a,
      #|  false,
      #|  1.5,
      #|  Some(1)
      #|)
    ),
  )
  let tuple = (1, 'a', false, 1.5, Some(1), "hello")
  inspect(
    pretty(tuple),
    content=(
      #|(1, a, false, 1.5, Some(1), "hello")
    ),
  )
  inspect(
    render(tuple, width=10),
    content=(
      #|(
      #|  1,
      #|  a,
      #|  false,
      #|  1.5,
      #|  Some(1),
      #|  "hello"
      #|)
    ),
  )
  let tuple = (1, 'a', false, 1.5, Some(1), "hello", 5)
  inspect(
    pretty(tuple),
    content=(
      #|(1, a, false, 1.5, Some(1), "hello", 5)
    ),
  )
  inspect(
    render(tuple, width=10),
    content=(
      #|(
      #|  1,
      #|  a,
      #|  false,
      #|  1.5,
      #|  Some(1),
      #|  "hello",
      #|  5
      #|)
    ),
  )
  let tuple = (1, 'a', false, 1.5, Some(1), "hello", 5, 6)
  inspect(
    pretty(tuple),
    content=(
      #|(1, a, false, 1.5, Some(1), "hello", 5, 6)
    ),
  )
  inspect(
    render(tuple, width=10),
    content=(
      #|(
      #|  1,
      #|  a,
      #|  false,
      #|  1.5,
      #|  Some(1),
      #|  "hello",
      #|  5,
      #|  6
      #|)
    ),
  )
  let tuple = (1, 'a', false, 1.5, Some(1), "hello", 5, 6, 7)
  inspect(
    pretty(tuple),
    content=(
      #|(1, a, false, 1.5, Some(1), "hello", 5, 6, 7)
    ),
  )
  inspect(
    render(tuple, width=10),
    content=(
      #|(
      #|  1,
      #|  a,
      #|  false,
      #|  1.5,
      #|  Some(1),
      #|  "hello",
      #|  5,
      #|  6,
      #|  7
      #|)
    ),
  )
}
