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
// - https://gitlab.inria.fr/fpottier/menhir/-/blob/809af6503305acaf6d9877e67c40ef651452e115/src/item.ml
//
// Original copyright notice copied below
//
// Copyright Inria. All rights reserved. This file is distributed under
// the terms of the GNU General Public License version 2, as described in
// the file LICENSE.

///|
priv struct Node[LookaheadSet] {
  item : EncodedLR0Item
  partial_lookahead_set : EpsilonLookaheadSet[LookaheadSet]
  epsilon_transitions : Array[Node[LookaheadSet]]
  // Below are transient states
  mut stamp : @stamp.T
  mut predecessors : @list.List[Node[LookaheadSet]]
  mut lookahead_set : LookaheadSet
}

///|
pub fn[LookaheadSet : AbstractLookaheadSet] build_closure_fn(
  grammar : @grm.Grammar,
) -> (Iter[(EncodedLR0Item, LookaheadSet)]) -> Array[
  (EncodedLR0Item, LookaheadSet),
] {
  let first = compute_first_fn(grammar)
  let node_by_item : @hashmap2.T[EncodedLR0Item, Node[LookaheadSet]] = @hashmap2.new(
    capacity=16,
  )
  for production in grammar.productions {
    for dot in 0..=production.rhs.length() {
      let item = EncodedLR0Item::new(production, dot)
      let partial_lookahead_set = if dot < production.rhs.length() {
        first(production.rhs[dot + 1:])
      } else {
        { set: LookaheadSet::empty(), nullable: false }
      }
      let node = {
        item,
        partial_lookahead_set,
        epsilon_transitions: [],
        stamp: @stamp.initial(),
        predecessors: @list.empty(),
        lookahead_set: LookaheadSet::empty(),
      }
      node_by_item.set(item, node)
    }
  }
  for pair in node_by_item {
    let node = pair.1
    match node.item.decode_postdot(grammar) {
      None | Some(T(_)) => ()
      Some(NT(nt)) =>
        for production in nt.productions {
          let encoded_item = EncodedLR0Item::new(production, 0)
          node.epsilon_transitions.push(node_by_item.get(encoded_item).unwrap())
        }
    }
  }
  fn(item_set) {
    let stamp = @stamp.new()
    let nodes = []
    letrec visit = fn(
      node : Node[LookaheadSet],
      lookahead_set : EpsilonLookaheadSet[LookaheadSet],
      parent : Node[LookaheadSet],
    ) {
      if lookahead_set.nullable {
        node.predecessors = @list.cons(parent, node.predecessors)
      }
      if node.stamp == stamp {
        node.lookahead_set = node.lookahead_set.union(lookahead_set.set)
      } else {
        node.lookahead_set = lookahead_set.set
        follow(node)
      }
    }
    and follow = fn(node : Node[LookaheadSet]) -> Unit {
      node.stamp = stamp
      nodes.push(node)
      for epsilon_transition in node.epsilon_transitions {
        visit(epsilon_transition, node.partial_lookahead_set, node)
      }
    }

    for item in item_set {
      let (item, lookahead_set) = item
      let node = node_by_item.get(item).unwrap()
      node.lookahead_set = lookahead_set
      follow(node)
    }

    // Populate the lookahead sets
    let stamp = @stamp.new()
    fn walk(node : Node[LookaheadSet]) {
      if node.stamp != stamp {
        node.stamp = stamp
        for predecessor in node.predecessors {
          walk(predecessor)
          node.lookahead_set = node.lookahead_set.union(
            predecessor.lookahead_set,
          )
        }
      }
    }

    for node in nodes {
      walk(node)
    }
    let closure = nodes.map(fn(node) { (node.item, node.lookahead_set) })
    // Cleanup the transient states
    for node in nodes {
      node.predecessors = @list.empty()
      node.lookahead_set = LookaheadSet::empty()
    }
    closure.sort_by_key(fn(pair) { pair.0 })
    closure
  }
}
