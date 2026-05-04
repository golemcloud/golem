// Copyright 2025 International Digital Economy Academy
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

///|
/// Create an empty stack.
///
/// # Example
///
/// ```
/// let s : Stack[Int] = @stack.Stack::new()
/// assert_true(s.is_empty())
/// let s1 : Stack[Int] = @stack.new()
/// assert_true(s1.is_empty())
/// ```
#as_free_fn
pub fn[T] Stack::new() -> Stack[T] {
  { elements: @list.List::new(), len: 0 }
}

///|
test "new" {
  let s : Stack[Int] = new()
  inspect(s, content="Stack::[]")
}

///|
/// Create a stack based on all elements in array.
///
/// # Example
///
/// ```
///   let s = Stack::from_array([1, 2, 3])
///   assert_eq(s.length(), 3)
/// ```
pub fn[T] Stack::from_array(array : Array[T]) -> Stack[T] {
  { elements: @list.from_array(array), len: array.length() }
}

///|
test "from_array" {
  let s = Stack::from_array([1, 2, 3])
  inspect(s, content="Stack::[1, 2, 3]")
  inspect(s.len, content="3")
}

///|
pub fn[T] Stack::from_iter(iter : Iter[T]) -> Stack[T] {
  let mut len = 0
  let elements = @list.from_iter(iter.tap(_ => len += 1))
  { elements, len }
}

///|
test "from_iter" {
  let s = Stack::from_iter([1, 2, 3].iter())
  inspect(s, content="Stack::[1, 2, 3]")
  inspect(s.len, content="3")
}

///|
/// Create a stack based on all elements in array.
///
/// # Example
///
/// ```
/// let s = of([1, 2, 3])
/// assert_eq(s.length(), 3)
/// ```
#as_free_fn
pub fn[T] Stack::of(array : FixedArray[T]) -> Stack[T] {
  { elements: @list.from_array(array), len: array.length() }
}

///|
test "of" {
  let s = of([1, 2, 3])
  inspect(s, content="Stack::[1, 2, 3]")
}

///|
pub impl[T : Show] Show for Stack[T] with output(self, logger : &Logger) -> Unit {
  logger.write_string("Stack::[")
  let mut i = 0
  self.elements.each(fn(t) {
    if i > 0 {
      logger.write_string(", ")
    }
    t.output(logger)
    i = i + 1
  })
  logger.write_string("]")
}

///|
test "to_string" {
  let empty : Stack[Int] = new()
  inspect(empty, content="Stack::[]")
  inspect(of([1, 2, 3, 4, 5]), content="Stack::[1, 2, 3, 4, 5]")
}

///|
/// Create a stack based on another stack.
///
/// # Example
///
/// ```
///   let s = @stack.of([1, 2, 3])
///   let s1 = Stack::from_stack(s)
///   assert_eq(s1.length(), 3)
/// ```
pub fn[T] Stack::from_stack(other : Stack[T]) -> Stack[T] {
  { ..other }
}

///|
test "from_stack" {
  let s = of([1, 2, 3])
  let s1 = Stack::from_stack(s)
  inspect(s1.elements == s.elements, content="true")
}

///|
/// Clear all elements in Stack
/// 
/// # Example
///
/// ```
///   let s = @stack.of([1, 2, 3])
///   s.clear()
///   assert_eq(s.length(), 0)
/// ```
pub fn[T] Stack::clear(self : Stack[T]) -> Unit {
  self.elements = @list.empty()
  self.len = 0
}

///|
test "clear" {
  let s = of([1, 2, 3])
  s.clear()
  inspect(s, content="Stack::[]")
}

///|
/// Same as the `clear()`, but returns an cleared stack
///
/// # Example
///
/// ```
///   let s = @stack.of([1, 2, 3]).return_with_clear()
///   assert_eq(s.length(), 0)
/// ```
pub fn[T] Stack::return_with_clear(self : Stack[T]) -> Stack[T] {
  self.clear()
  self
}

///|
test "return_with_clear" {
  let s = of([1, 2, 3]).return_with_clear()
  inspect(s, content="Stack::[]")
}

///|
/// Push an element into the stack.
/// 
/// # Example
///
/// ```
///   let s = @stack.new()
///   s.push(1)
///   assert_eq(s.length(), 1)
/// ```
pub fn[T] Stack::push(self : Stack[T], x : T) -> Unit {
  self.elements = @list.cons(x, self.elements)
  self.len = self.len + 1
}

///|
test "push" {
  let s = new()
  s.push(1)
  inspect(s, content="Stack::[1]")
  inspect(s.len, content="1")
}

///|
/// Push other stack into the current stack.
/// 
/// # Example
///
/// ```
///   let s = @stack.of([1, 2, 3])
///   let s1 : Stack[Int] = @stack.new()
///   s1.push_stack(s)
///   assert_eq(s1.length(), 3)
/// ```
pub fn[T] Stack::push_stack(self : Stack[T], stack : Stack[T]) -> Unit {
  stack.elements.iter().each(fn(i) { self.push(i) })
}

///|
test "push_stack" {
  let s = of([1, 2, 3])
  let s1 : Stack[Int] = new()
  s1.push_stack(s)
  inspect(s1, content="Stack::[3, 2, 1]")
  inspect(s.length() == s1.length(), content="true")
}

///|
/// Push an array into the stack.
/// 
/// # Example
///
/// ```
///   let s : Stack[Int] = @stack.new()
///   s.push_array([1, 2, 3])
///   assert_eq(s.length(), 3)
/// ```
pub fn[T] Stack::push_array(self : Stack[T], array : Array[T]) -> Unit {
  array.each(fn(i) { self.push(i) })
}

///|
test "push_array" {
  let s : Stack[Int] = new()
  s.push_array([1, 2, 3])
  inspect(s, content="Stack::[3, 2, 1]")
  inspect(s.len, content="3")
}

///|
/// Pop an element from the top of the stack.
/// If there are elements in the stack, return `Some (the top element of the stack)`, otherwise return `None`.
///
/// # Example
///
/// ```
///   let s = @stack.of([1, 2, 3])
///   let s1 : Stack[Int] = @stack.new()
///   assert_eq(s.pop(), Some(1))
///   assert_eq(s.length(), 2)
///   assert_eq(s1.pop(), None)
/// ```
pub fn[T] Stack::pop(self : Stack[T]) -> T? {
  match self.elements {
    More(hd, tail=tl) => {
      self.elements = tl
      self.len = self.len - 1
      Some(hd)
    }
    Empty => None
  }
}

///|
test "pop" {
  let s = of([1, 2, 3])
  let s1 : Stack[Int] = new()
  inspect(s.pop(), content="Some(1)")
  inspect(s, content="Stack::[2, 3]")
  inspect(s.len, content="2")
  inspect(s1.pop(), content="None")
  inspect(s1, content="Stack::[]")
  inspect(s1.len, content="0")
}

///|
/// Pop an element from the top of the stack.
/// If there are elements in the stack, return the top element of the stack, otherwise abort.
///
/// @alert unsafe "Panic if the stack is empty."
pub fn[T] Stack::unsafe_pop(self : Stack[T]) -> T {
  match self.elements {
    More(hd, tail=tl) => {
      self.elements = tl
      self.len = self.len - 1
      hd
    }
    Empty => abort("pop of empty stack")
  }
}

///|
test "unsafe_pop" {
  let s = of([1, 2, 3])
  inspect(s.unsafe_pop(), content="1")
  inspect(s, content="Stack::[2, 3]")
  inspect(s.len, content="2")
}

///|
/// Drop the element at the top of the stack.
/// Like pop, but does not return elements and does nothing if the Stack is empty.
///
/// # Example
///
/// ```
///   let s = @stack.of([1, 2, 3])
///   s.drop()
///   assert_eq(s.length(), 2)
/// ```
pub fn[T] Stack::drop(self : Stack[T]) -> Unit {
  match self.elements {
    More(_hd, tail=tl) => {
      self.elements = tl
      self.len = self.len - 1
    }
    Empty => ()
  }
}

///|
test "drop" {
  let s = of([1, 2, 3])
  s.drop()
  inspect(s, content="Stack::[2, 3]")
  inspect(s.len, content="2")
}

///|
/// Drop the element at the top of the stack.
/// Like drop, but when the drop is successful, it returns `Ok(())`, and when it fails, it returns `Err(())`
///
/// # Example
///
/// ```
///   let s = @stack.of([1, 2, 3])
///   let r = s.drop_result() // Ok(())
///   assert_eq(r, Ok(()))
/// ```
pub fn[T] Stack::drop_result(self : Stack[T]) -> Result[Unit, Unit] {
  match self.elements {
    More(_hd, tail=tl) => {
      self.elements = tl
      self.len = self.len - 1
      Ok(())
    }
    Empty => Err(())
  }
}

///|
test "drop_result" {
  let s = of([1, 2, 3])
  let s1 : Stack[Int] = new()
  inspect(s1.drop_result() == Err(()), content="true")
  inspect(s.drop_result() == Ok(()), content="true")
  inspect(s, content="Stack::[2, 3]")
  inspect(s.len, content="2")
}

///|
/// Only the top element of the stack is returned and will not be pop or drop.
/// If there are elements in the stack, return `Some (the top element of the stack)`, otherwise return `None`.
///
/// # Example
///
/// ```
///   let s = Stack::from_array([1, 2, 3])
///   assert_eq(s.peek(), Some(1))
///   assert_eq(s.length(), 3)
/// ```
pub fn[T] Stack::peek(self : Stack[T]) -> T? {
  self.elements.head()
}

///|
test "peek" {
  let s = of([1, 2, 3])
  inspect(s.peek(), content="Some(1)")
  inspect(s, content="Stack::[1, 2, 3]")
  inspect(s.len, content="3")
}

///|
/// Only the top element of the stack is returned and will not be pop or drop.
/// If there are elements in the stack, return the top element of the stack, otherwise abort.
///
/// @alert unsafe "Panic if the stack is empty."
pub fn[T] Stack::unsafe_peek(self : Stack[T]) -> T {
  self.elements.unsafe_head()
}

///|
test "unsafe_peek" {
  let s : Stack[Int] = of([1, 2, 3])
  inspect(s.unsafe_peek(), content="1")
  inspect(s, content="Stack::[1, 2, 3]")
  inspect(s.len, content="3")
}

///|
/// If stack is empty, return true, otherwise return false.
///
/// # Example
///
/// ```
///   let s = @stack.of([1, 2, 3])
///   assert_false(s.is_empty())
///   let empty : Stack[Unit] = @stack.new()
///   assert_true(empty.is_empty())
/// ```
pub fn[T] Stack::is_empty(self : Stack[T]) -> Bool {
  self.len == 0
}

///|
test "is_empty" {
  let empty : Stack[Unit] = new()
  inspect(of([1, 2, 3]).is_empty(), content="false")
  inspect(empty.is_empty(), content="true")
}

///|
/// Returns the number of elements of the Stack
pub fn[T] Stack::length(self : Stack[T]) -> Int {
  self.len
}

///|
/// Iterates over the elements of the stack from top to bottom.
/// 
/// # Example
/// ```
///   let s = @stack.of([1, 2, 3])
///   let mut sum = 0
///   s.each(fn(i) { sum = sum + i })
///   assert_eq(sum, 6)
/// ```
pub fn[T] Stack::each(self : Stack[T], f : (T) -> Unit) -> Unit {
  self.elements.iter().each(f)
}

///|
test "iter" {
  let s : Stack[Int] = new()
  let mut sum = 0
  let mut sub = 0
  s.each(fn(i) { sum = sum + i })
  inspect(sum, content="0")
  s.push(1)
  s.push(2)
  s.push(3)
  inspect(s.unsafe_peek(), content="3")
  sum = 0
  sub = s.unsafe_peek()
  s.each(fn(i) { sum = sum + i })
  s.each(fn(i) { sub = sub - i }) // 3 - 3 - 2 - 1
  inspect(sum, content="6")
  inspect(sub, content="-3")
}

///|
/// Folds over the elements of the stack from top to bottom.
/// 
/// # Example
/// ```
///   let s = @stack.of([1, 2, 3])
///   let sum = s.fold(init=0, fn(acc, i) { acc + i })
///   assert_eq(sum, 6)
/// ```
pub fn[T, U] Stack::fold(self : Stack[T], init~ : U, f : (U, T) -> U) -> U {
  self.elements.fold(init~, f)
}

///|
test "fold" {
  let s = of([1, 2, 3])
  let sum = s.fold(init=0, fn(acc, i) { acc + i })
  inspect(sum, content="6")
}

///|
/// Covert stack to an iterator.
/// 
/// # Example
/// ```
/// let stack = @stack.new()
/// 
/// stack.push(3)
/// stack.push(2)
/// stack.push(1)
/// inspect(stack.iter(), content="[1, 2, 3]")
/// ```
pub fn[T] Stack::iter(self : Stack[T]) -> Iter[T] {
  self.elements.iter()
}

///|
/// Convert stack to array.
///
/// # Example
///
/// ```
///   assert_eq(@stack.of([1, 2, 3]).to_array(), [1, 2, 3])
/// ```
pub fn[T] Stack::to_array(self : Stack[T]) -> Array[T] {
  self.elements.to_array()
}

///|
test "to_array" {
  inspect(of([3, 2, 1]).to_array(), content="[3, 2, 1]")
}

///|
/// Compare two stacks.
///
/// NOTE: Since the current standard library @immut/list.T lacks the equal or op_equal function, 
/// this function internally implements the equal function of @immut/list.T.
///
/// # Example
///
/// ```
///   assert_true(@stack.of([2, 4, 6]).equal(@stack.of([2, 4, 6])))
/// ```
pub fn[T : Eq] Stack::equal(self : Stack[T], other : Stack[T]) -> Bool {
  if self.len == other.len {
    self.elements == other.elements
  } else {
    false
  }
}

///|
pub fn[T : Eq] Stack::op_equal(self : Stack[T], other : Stack[T]) -> Bool {
  self.equal(other)
}

///|
test "equal" {
  inspect(of([2, 4, 6]).equal(of([2, 4, 6])), content="true")
  inspect(of([2, 4, 6]).equal(of([2, 4, 7])), content="false")
}
