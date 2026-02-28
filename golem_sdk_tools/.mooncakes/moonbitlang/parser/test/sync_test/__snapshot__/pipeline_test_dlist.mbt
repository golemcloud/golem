fn init {
  let dl = from_array([ 2, 4, 6 ])
  dl.push(1)
  dl.next() |> ignore
  dl.next() |> ignore
  dl.push(3)
  dl.next() |> ignore
  dl.next() |> ignore
  dl.push(5)
  while dl.prev() {}
  dl.iter(println)
}

// a doubly linked list
enum Node[X] {
  Nil
  Link(
    mut elem~ : X,
    mut prev~ : Node[X],
    mut next~ : Node[X]
  )
}

fn[X] Node::set_next(self : Node[X], next : Node[X]) -> Unit {
  match self {
    Node::Nil => ()
    Link(_) as node => node.next = next
  }
}

fn[X] Node::set_prev(self : Node[X], prev : Node[X]) -> Unit {
  match self {
    Node::Nil => ()
    Link(_) as node => node.prev = prev
  }
}

struct DList[X] {
  mut head : Node[X]
}

fn[X] empty() -> DList[X] {
  { head: Node::Nil }
}

fn[X] singleton(elem : X) -> DList[X] {
  { head: Link(elem~, prev=Node::Nil, next=Node::Nil) }
}

fn[X] DList::push(self : DList[X], elem : X) -> Unit {
  match self.head {
    Node::Nil => self.head = Link(elem~, prev=Node::Nil, next=Node::Nil)
    Link(_) as prev_head => {
      let new_head : Node[X] = Link(elem~, prev=prev_head.prev, next=prev_head)
      prev_head.prev.set_next(new_head)
      prev_head.prev = new_head
      self.head = new_head
    }
  }
}

fn[X] DList::pop(self : DList[X]) -> X? {
  match self.head {
    Node::Nil => None
    Link(_) as head => {
      head.prev.set_next(head.next)
      head.next.set_prev(head.prev)
      self.head = match head.next { Node::Nil => head.next; Link(_) => head.prev }
      head.prev = Node::Nil
      head.next = Node::Nil
      Some(head.elem)
    }
  }
}

fn[X] DList::next(self : DList[X]) -> Bool {
  match self.head {
    Node::Nil | Link(next=Node::Nil, ..) => false
    Link(_) as head => {
      self.head = head.next
      true
    }
  }
}

fn[X] DList::prev(self : DList[X]) -> Bool {
  match self.head {
    Node::Nil | Link(prev=Node::Nil, ..) => false
    Link(_) as head => {
      self.head = head.prev
      true
    }
  }
}

fn[X] DList::iter(self : DList[X], f : (X) -> Unit) -> Unit {
  let mut i = 10
  loop self.head {
    Node::Nil => ()
    Link(elem~, next~, ..) => {
      f(elem)
      continue next
    }
  }
}

fn[X] from_array(arr : FixedArray[X]) -> DList[X] {
  let result = empty()
  for i = arr.length(); i > 0; i = i - 1 {
    result.push(arr[i - 1])
  }
  result
}
