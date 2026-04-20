# Rational

The `Rational` type represents a rational number, which is a number that can be expressed as a fraction `a/b` where `a` and `b` are integers and `b` is not zero.

# Usage

## Arithmetic Operations

The `Rational` type supports the following arithmetic operations:

```moonbit check
///|
test {
  let a = @rational.new(1L, 2L).unwrap()
  let b = @rational.new(1L, 3L).unwrap()
  assert_eq(a + b, @rational.new(5L, 6L).unwrap())
  assert_eq(a - b, @rational.new(1L, 6L).unwrap())
  assert_eq(a * b, @rational.new(1L, 6L).unwrap())
  assert_eq(a / b, @rational.new(3L, 2L).unwrap())
  assert_eq(-a, @rational.new(-1L, 2L).unwrap())
  assert_eq(a.reciprocal(), @rational.new(2L, 1L).unwrap())
  assert_eq(a.abs(), @rational.new(1L, 2L).unwrap())
}
```

## Comparison Operations

The `Rational` type supports the following comparison operations:

```moonbit check
///|
test {
  let a = @rational.new(1L, 2L).unwrap()
  let b = @rational.new(1L, 3L).unwrap()
  assert_eq(a == b, false)
  assert_eq(a != b, true)
  assert_eq(a < b, false)
  assert_eq(a <= b, false)
  assert_eq(a > b, true)
  assert_eq(a >= b, true)
  assert_eq(a.compare(b), 1)
}
```

## Integer Operations

The `Rational` type supports the following integer operations:

```moonbit check
///|
test {
  let a = @rational.new(1L, 2L).unwrap()
  assert_eq(a.floor(), 0)
  assert_eq(a.ceil(), 1)
  assert_eq(a.fract().to_string(), "1/2")
  assert_eq(a.trunc(), 0)
  assert_eq(a.is_integer(), false)
}
```

## Double Operations

The `Rational` type supports the following double operations:

```moonbit check
///|
test {
  let a = @rational.new(1L, 2L).unwrap()
  assert_eq(a.to_double(), 0.5)
  assert_eq(@rational.from_double(0.5).to_string(), "1/2")
}
```

## String Operations

The `Rational` type supports the following string operations:

```moonbit check
///|
test {
  let a = @rational.new(1L, 2L).unwrap()
  assert_eq(a.to_string(), "1/2")
}
```