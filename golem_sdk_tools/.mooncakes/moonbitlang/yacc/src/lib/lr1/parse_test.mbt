// Copyright (C) 2025 International Digital Economy Academy
//
// This program is free software; you can redistribute it and/or
// modify it under the terms of the GNU General Public License
// as published by the Free Software Foundation; version 2.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program; if not, see <https://www.gnu.org/licenses/>.

///|
type Obj = Error

///|
struct InputProvider {
  read : () -> (@grm.Terminal, Obj)?
}

///|
struct ProductionActionProvider {
  get_action : (Int) -> (ArrayView[Obj]) -> Obj
}

///|
pub fn parse(
  grammar : @grm.Grammar,
  input_provider : InputProvider,
  action_provider : ProductionActionProvider,
) -> Obj {
  let automaton = @lr1.Automaton::build(grammar, user_eoi=false)
  let state_stack = []
  let data_stack = []
  fn read_as_input() -> (@lr1.Lookahead, Obj) {
    match (input_provider.read)() {
      None => (EndOfInput, ObjUnit(()))
      Some((terminal, obj)) => (Input(terminal), obj)
    }
  }

  let mut lookahead = read_as_input()
  state_stack.push(automaton.starts[0].1)
  while true {
    let state = state_stack.last().unwrap()
    // println("state \{state.num}")
    // for item in state.items {
    //   println(item)
    // }
    // println("symbol \{Show::to_string(lookahead)}")
    match state.action.get(lookahead.0) {
      Some(Accept) => return data_stack.unsafe_pop()
      Some(Shift(next_state)) => {
        state_stack.push(next_state)
        data_stack.push(lookahead.1)
        lookahead = read_as_input()
      }
      Some(Reduce(production)) => {
        let args = data_stack[data_stack.length() - production.rhs.length():]
        let result = (action_provider.get_action)(production.num)(args)
        for i in 0..<production.rhs.length() {
          let _ = state_stack.pop()
          let _ = data_stack.pop()

        }
        data_stack.push(result)
        let next_state = state_stack.last().unwrap().goto.get(
          NT(production.lhs),
        )
        guard next_state is Some(next_state)
        state_stack.push(next_state)
      }
      Some(Conflict(_)) => panic()
      None => panic()
    }
  }
  panic()
}

///|
suberror ObjNumber {
  ObjNumber(Int)
}

///|
suberror ObjUnit {
  ObjUnit(Unit)
}

///|
fn make_action_provider(
  actions : Map[Int, (ArrayView[Obj]) -> Obj],
) -> ProductionActionProvider {
  { get_action: fn(num) { actions.get(num).unwrap() } }
}

///|
fn make_input_provider(
  tokens : Array[(String, Obj)],
  terminal_by_name : @sorted_map.SortedMap[String, @grm.Terminal],
) -> InputProvider {
  let mut i = 0
  {
    read: fn() {
      if i >= tokens.length() {
        return None
      }
      let (name, obj) = tokens[i]
      let terminal = terminal_by_name.get(name).unwrap()
      i += 1
      return Some((terminal, obj))
    },
  }
}

///|
test "parse" {
  let actions : Map[Int, (ArrayView[Obj]) -> Obj] = {}
  let grammar = @grm.build(fn(ctx) {
    let prod1 = ctx.add_production("Start", ["Add"])
    actions[prod1] = fn(args) {
      guard args is [arg1]
      return arg1
    }
    let prod2 = ctx.add_production("Add", ["Add", "+", "Factor"])
    actions[prod2] = fn(args) {
      guard args is [ObjNumber(arg1), _, ObjNumber(arg2)]
      return ObjNumber(arg1 + arg2)
    }
    let prod2_5 = ctx.add_production("Add", ["Add", "-", "Factor"])
    actions[prod2_5] = fn(args) {
      guard args is [ObjNumber(arg1), _, ObjNumber(arg2)]
      return ObjNumber(arg1 - arg2)
    }
    let prod3 = ctx.add_production("Add", ["Factor"])
    actions[prod3] = fn(args) {
      guard args is [arg1]
      return arg1
    }
    let prod4 = ctx.add_production("Factor", ["Factor", "*", "Term"])
    actions[prod4] = fn(args) {
      guard args is [ObjNumber(arg1), _, ObjNumber(arg2)]
      return ObjNumber(arg1 * arg2)
    }
    let prod5 = ctx.add_production("Factor", ["Term"])
    actions[prod5] = fn(args) {
      guard args is [arg1]
      return arg1
    }
    let prod6 = ctx.add_production("Term", ["(", "Add", ")"])
    actions[prod6] = fn(args) {
      guard args is [_, arg, _]
      return arg
    }
    let prod7 = ctx.add_production("Term", ["Number"])
    actions[prod7] = fn(args) {
      guard args is [arg1]
      return arg1
    }
    ctx.set_start("Start")
  })
  let terminal_by_name = @sorted_map.from_iter(
    grammar.terminals.iter().map(fn(t) { (t.name, t) }),
  )
  let tokens : Array[(String, Obj)] = [
    ("Number", ObjNumber(3)),
    ("+", ObjUnit(())),
    ("(", ObjUnit(())),
    ("Number", ObjNumber(2)),
    ("-", ObjUnit(())),
    ("Number", ObjNumber(5)),
    (")", ObjUnit(())),
    ("*", ObjUnit(())),
    ("Number", ObjNumber(0)),
  ]
  let input_provider = make_input_provider(tokens, terminal_by_name)
  let action_provider = make_action_provider(actions)
  let result = parse(grammar, input_provider, action_provider)
  guard result is ObjNumber(n)
  assert_eq(n, 3)
}
