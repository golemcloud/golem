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

// Adapted from Menhir
// - https://gitlab.inria.fr/fpottier/menhir/-/blob/809af6503305acaf6d9877e67c40ef651452e115/src/lr0.ml
// - https://gitlab.inria.fr/fpottier/menhir/-/blob/809af6503305acaf6d9877e67c40ef651452e115/src/LR1Pager.ml
//
// Original copyright notice copied below
//
// Copyright Inria. All rights reserved. This file is distributed under
// the terms of the GNU General Public License version 2, as described in
// the file LICENSE.

///|
pub(all) struct LR1ItemGroup {
  core : LR0Item
  lookahead_set : Iter[Lookahead]
}

///|
pub impl Show for LR1ItemGroup with output(self, logger) {
  logger.write_string("[")
  logger.write_object(self.core.production.num)
  logger.write_string(", ")
  self.core.production.output_with_opt_dot(logger, dot=self.core.dot)
  logger.write_string(", ")
  let mut first = true
  for lookahead in self.lookahead_set {
    if not(first) {
      logger.write_string(" / ")
    }
    logger.write_object(lookahead)
    first = false
  }
  logger.write_string("]")
}

///|
pub(all) enum Decision {
  Shift(LR1State)
  Reduce(@grm.Production)
  Accept
  // Error
  Conflict(@list.List[Decision])
} derive(Eq)

///|
priv struct LR0Node {
  num : Int
  closure_items : EncodedLR0ItemSet
  closure_symbolic_lookahead_set : Array[EncodedSymbolicLookaheadSet]
}

///|
impl Eq for LR0Node with equal(self, other) {
  self.num == other.num
}

///|
priv struct LR1PreState {
  items : EncodedLR1ItemSet
  mut state : LR1State?
}

///|
pub(all) struct LR1State {
  // Used for decoding the LR1ItemSet
  grammar : @grm.Grammar
  mut num : Int
  items : EncodedLR1ItemSet
  goto : @sorted_map.SortedMap[@grm.Symbol, LR1State]
  action : @sorted_map.SortedMap[Lookahead, Decision]
  mut stamp : @stamp.T
}

///|
pub impl Eq for LR1State with equal(self, other) {
  self.num == other.num
}

///|
pub impl Compare for LR1State with compare(self, other) {
  self.num.compare(other.num)
}

///|
pub impl Hash for LR1State with hash_combine(self, hasher) {
  hasher.combine_int(self.num)
}

///|
pub fn LR1State::iter_item_groups(self : LR1State) -> Iter[LR1ItemGroup] {
  self.items.decode_item_groups(self.grammar)
}

///|
pub fn LR1State::set_action(
  self : LR1State,
  input : Lookahead,
  decision : Decision,
) -> Unit {
  match self.action.get(input) {
    Some(Conflict(decisions)) =>
      if not(decisions.contains(decision)) {
        self.action[input] = Conflict(@list.cons(decision, decisions))
      }
    Some(decision0) =>
      if decision0 != decision {
        self.action[input] = Conflict(
          @list.cons(decision, @list.cons(decision0, @list.empty())),
        )
      }
    None => self.action[input] = decision
  }
}

///|
pub(all) struct ConflictLocation {
  state : LR1State
  input : Lookahead
} derive(Eq, Hash)

///|
pub(all) struct Automaton {
  states : Array[LR1State]
  starts : Array[(@grm.Production, LR1State)]
  // The resolved decision will be stored in the state's action table.
  // Here records the original decisions that lead to the conflict.
  conflicts : @hashmap2.T[ConflictLocation, @list.List[Decision]]
}

///|
pub fn Automaton::build(grammar : @grm.Grammar, user_eoi~ : Bool) -> Automaton {
  let closure : (Iter[(EncodedLR0Item, EncodedSymbolicLookaheadSet)]) -> Array[
    (EncodedLR0Item, EncodedSymbolicLookaheadSet),
  ] = build_closure_fn(grammar)
  let node_by_kernel_items : @hashmap2.T[EncodedLR0ItemSet, LR0Node] = @hashmap2.new(
    capacity=65536,
  )
  let node_transitions : Array[
    @hashmap2.T[@grm.Symbol, (LR0Node, Array[EncodedSymbolicLookaheadSet])],
  ] = []
  let node_reductions : Array[
    Array[(EncodedSymbolicLookaheadSet, @grm.Production)],
  ] = []
  let node_starts : Array[(LR0Node, @grm.Production)] = []

  //
  fn get_reductions(
    symbolic_items : Array[(EncodedLR0Item, EncodedSymbolicLookaheadSet)],
  ) -> Array[(EncodedSymbolicLookaheadSet, @grm.Production)] {
    let result = []
    for symbolic_item in symbolic_items {
      let (item, symbolic_lookahead_set) = symbolic_item
      match item.decode_postdot(grammar) {
        Some(_) => ()
        None =>
          result.push((symbolic_lookahead_set, item.decode_production(grammar)))
      }
    }
    result
  }

  //
  fn transitions(
    symbolic_items : Array[(EncodedLR0Item, EncodedSymbolicLookaheadSet)],
  ) {
    let trans : @hashmap2.T[
      @grm.Symbol,
      (Array[EncodedLR0Item], Array[EncodedSymbolicLookaheadSet]),
    ] = @hashmap2.new(capacity=16)
    for entry in symbolic_items {
      let (item, symbolic_lookahead_set) = entry
      match item.decode_postdot(grammar) {
        None => ()
        Some(symbol) => {
          let next_item = item.unsafe_shift()
          let (kernel_items, symbolic_lookahead_set_table) = trans.get_or_init(
            symbol,
            fn(_arg) { ([], []) },
          )
          kernel_items.push(next_item)
          symbolic_lookahead_set_table.push(symbolic_lookahead_set)
        }
      }
    }
    trans
  }

  fn explore(kernel_items : EncodedLR0ItemSet) -> LR0Node {
    let mut defer_ = None
    let node = node_by_kernel_items.get_or_init(kernel_items, fn(kernel_items) {
      let symbolic_items = closure(
        kernel_items.items
        .mapi(fn(index, item) {
          (item, EncodedSymbolicLookaheadSet::variable(index))
        })
        .iter(),
      )
      let node : LR0Node = {
        num: node_by_kernel_items.size(),
        closure_items: EncodedLR0ItemSet::from_sorted_array(
          symbolic_items.map(fn(item) { item.0 }),
        ),
        closure_symbolic_lookahead_set: symbolic_items.map(fn(item) { item.1 }),
      }
      defer_ = Some(symbolic_items)
      node
    })
    match defer_ {
      Some(symbolic_items) => {
        node_reductions.push(get_reductions(symbolic_items))
        node_transitions.push(@hashmap2.new(capacity=16))
        for tran in transitions(symbolic_items) {
          let (symbol, (kernel_items, kernel_symbolic_lookahead_set_table)) = tran
          let target = explore(
            EncodedLR0ItemSet::from_sorted_array(kernel_items),
          )
          node_transitions[node.num].set(
            symbol,
            (target, kernel_symbolic_lookahead_set_table),
          )
        }
      }
      None => ()
    }
    node
  }

  for start_production in grammar.starts {
    node_starts.push(
      (
        explore(
          EncodedLR0ItemSet::from_sorted_array([
            EncodedLR0Item::new(start_production, 0),
          ]),
        ),
        start_production,
      ),
    )
  }
  let mut next_state_num = 0
  let starts : Array[(@grm.Production, LR1PreState)] = []
  let families : FixedArray[Array[LR1PreState]] = FixedArray::makei(
    node_by_kernel_items.size(),
    fn(_i) { [] },
  )
  let queue = []
  for entry in node_starts {
    let (node, start_production) = entry
    let start_state = LR1PreState::{
      items: {
        core: node,
        kernel_lookahead_set_table: [EncodedLookaheadSet::end_of_input()],
      },
      state: None,
    }
    families[start_state.items.core.num].push(start_state)
    starts.push((start_production, start_state))
    queue.push(start_state)
  }
  letrec examine = fn(candidate : LR1PreState) -> Unit {
    let family = families[candidate.items.core.num]
    for state in family {
      if candidate.items.subsume(state.items) {
        return ()
      }
    }
    fuse(family, candidate)
  }
  and fuse = fn(family : Array[LR1PreState], candidate : LR1PreState) -> Unit {
    for state in family {
      if state.items.weak_compat(candidate.items) &&
        (if user_eoi { state.items.eoi_compact(candidate.items) } else { true }) {
        let new_state = LR1PreState::{
          items: candidate.items.merge(state.items),
          state: None,
        }
        match family.search_by(fn(it) { physical_equal(it, state) }) {
          Some(index) => family.remove(index) |> ignore
          None => ()
        }
        return fuse(family, new_state)
      }
    } else {
      family.push(candidate)
      queue.push(candidate)
    }
  }

  while not(queue.is_empty()) {
    let state = queue.unsafe_pop()
    let trans = node_transitions[state.items.core.num]
    for entry in trans {
      let (_symbol, (target_node, kernel_symbolic_lookahead_set_array)) = entry
      let kernel_lookahead_set_table = kernel_symbolic_lookahead_set_array.map(fn(
        symbolic_lookahead_set,
      ) {
        symbolic_lookahead_set.interpret(state.items.kernel_lookahead_set_table)
      })
      let target_state = LR1PreState::{
        items: { core: target_node, kernel_lookahead_set_table },
        state: None,
      }
      examine(target_state)
    }
  }
  let states : Array[LR1State] = []
  fn state_from_pre(pre_state : LR1PreState) -> LR1State {
    match pre_state.state {
      Some(state) => state
      None => {
        let state = LR1State::{
          grammar,
          num: next_state_num,
          items: pre_state.items,
          goto: @sorted_map.new(),
          action: @sorted_map.new(),
          stamp: @stamp.initial(),
        }
        next_state_num += 1
        pre_state.state = Some(state)
        state
      }
    }
  }

  let queue = []
  for entry in starts {
    let (_production, pre_state) = entry
    queue.push(state_from_pre(pre_state))
  }
  let stamp = @stamp.new()
  while not(queue.is_empty()) {
    let state = queue.unsafe_pop()
    if state.stamp == stamp {
      continue
    }
    state.stamp = stamp
    states.push(state)
    let trans = node_transitions[state.items.core.num]
    for entry in trans {
      let (symbol, (target_node, symbolic_lookahead_set_array)) = entry
      let family = families[target_node.num]
      let pre_state = match family {
        [] => panic()
        [target_state] => target_state
        family => {
          let kernel_lookahead_set_table = symbolic_lookahead_set_array.map(fn(
            symbolic_lookahead_set,
          ) {
            symbolic_lookahead_set.interpret(
              state.items.kernel_lookahead_set_table,
            )
          })
          let next_items = { core: target_node, kernel_lookahead_set_table }
          family
          .iter()
          .find_first(fn(state) { next_items.subsume(state.items) })
          .unwrap()
        }
      }
      let target_state = state_from_pre(pre_state)
      state.goto[symbol] = target_state
      if target_state.stamp != stamp {
        queue.push(target_state)
      }
    }
  }
  for index, state in states {
    state.num = index
  }

  // Populate actions
  let conflicts = @hashmap2.new(capacity=16)
  fn check_add_conflict(state : LR1State, input : Lookahead) {
    match state.action.get(input) {
      Some(Conflict(decisions)) => conflicts.set({ state, input }, decisions)
      Some(_) | None => ()
    }
  }

  for _, state in states {
    let core = state.items.core
    for tran in node_transitions[core.num] {
      let (symbol, _) = tran
      match symbol {
        NT(_) => ()
        T(term) => {
          state.set_action(Input(term), Shift(state.goto.get(T(term)).unwrap()))
          check_add_conflict(state, Input(term))
        }
      }
    }
    for reduction in node_reductions[core.num] {
      let (symbolic_lookahead_set, production) = reduction
      let lookahead_set = symbolic_lookahead_set.interpret(
        state.items.kernel_lookahead_set_table,
      )
      for item in core.closure_items.decode_iter(grammar) {
        for lookahead in lookahead_set.decode_iter(grammar) {
          if lookahead == EndOfInput &&
            grammar.starts.contains(production) &&
            item.1.dot == 1 {
            state.set_action(lookahead, Accept)
          } else {
            state.set_action(lookahead, Reduce(production))
          }
          check_add_conflict(state, lookahead)
        }
      }
    }
  }
  let starts = starts.map(fn(entry) {
    let (production, pre_state) = entry
    (production, state_from_pre(pre_state))
  })
  { states, starts, conflicts }
}
