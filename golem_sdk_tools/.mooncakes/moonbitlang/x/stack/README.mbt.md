# Stack

## Overview

Stack is a last in first out (LIFO) data structure, allowing to process their elements in the reverse order they come.

## Usage

### Create and Clear

The stack can be created with the `new` function, or by using the function with prefix `from` to create a stack from an existing collection.
For instance, `from_array` creates a stack from an array.

```moonbit check
///|
test {
  let st : @stack.Stack[Unit] = Stack::new()
  inspect(st, content="Stack::[]")
  let st2 = Stack::from_array([1, 2, 3])
  inspect(st2, content="Stack::[1, 2, 3]")
  let st3 = Stack::of([1, 2, 3])
  inspect(st3, content="Stack::[1, 2, 3]")
}
```

To clear the elements of the stack, use the `clear` method.

```moonbit check
///|
test {
  let st = @stack.Stack::from_array([1, 2, 3])
  st.clear()
  inspect(st, content="Stack::[]")
}
```

### Length

Use `length` to get the number of elements in the stack. The `is_empty` method can be used to check if the stack is empty.

```moonbit check
///|
test {
  let st = Stack::of([1, 2, 3])
  inspect(st.length(), content="3") // 3
  inspect(st.is_empty(), content="false") // false
}
```

### Pop and Push

To add elements to the stack, use the `push` method, and to remove them, use the `pop` method.

```moonbit check
///|
test {
  let st = Stack::new()
  st.push(1)
  st.push(2)
  inspect(st.pop(), content="Some(2)")
}
```

The unsafe version of `pop` is `unsafe_pop`, which will panic if the stack is empty.

```moonbit check
///|
test {
  let st = Stack::new()
  st.push(1)
  inspect(st.unsafe_pop(), content="1") // 1
}

///|
test "panic" {
  let st = Stack::new()
  st.unsafe_pop()
}
```

If you don't want to remove the element, you can use the `peek` method and the unsafe version `unsafe_peek`.

```moonbit check
///|
test {
  let st = Stack::of([1, 2, 3])
  inspect(st.peek(), content="Some(1)") // Some(1)
  inspect(st.unsafe_peek(), content="1") // 1
}
```

If the result of `pop` is not needed, you can use the `drop` method.

```moonbit check
///|
test {
  let st = Stack::of([1, 2, 3])
  st.drop()
  inspect(st, content="Stack::[2, 3]")
}
```

### Traverse

To traverse the stack, use the `iter` method.

```moonbit check
///|
test {
  let st = Stack::of([1, 2, 3])
  let mut sum = 0
  st.iter().each(fn(x) { sum += x })
  inspect(sum, content="6")
}
```

### Conversion

You can convert the stack to an array using the `to_array` method or the `iter` method.

```moonbit check
///|
test {
  let st = Stack::of([1, 2, 3])
  inspect(st.to_array(), content="[1, 2, 3]")
  inspect(Array::from_iter(st.iter()), content="[1, 2, 3]")
}
```
