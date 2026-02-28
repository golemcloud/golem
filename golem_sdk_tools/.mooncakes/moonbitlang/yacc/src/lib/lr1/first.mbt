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

// TODO: test this

///|
fn[T : AbstractLookaheadSet] compute_first_fn(
  grammar : @grm.Grammar,
) -> (ArrayView[@grm.Symbol]) -> EpsilonLookaheadSet[T] {
  let nt_first_map : Array[EpsilonLookaheadSet[T]] = Array::make(
    grammar.nonterminals.length(),
    { set: T::empty(), nullable: false },
  )
  fn first(symbols : ArrayView[@grm.Symbol]) {
    match symbols {
      [] => { set: T::empty(), nullable: true }
      [symbol, .. rest] =>
        (match symbol {
          T(t) => { set: T::singleton(Input(t)), nullable: false }
          NT(nt) => nt_first_map[nt.num]
        }) +
        first(rest)
    }
  }

  let mut fixedpoint_reached = false
  while not(fixedpoint_reached) {
    fixedpoint_reached = true
    for nt in grammar.nonterminals {
      let old_set = nt_first_map[nt.num]
      let new_set = nt.productions.fold(
        init={ set: T::empty(), nullable: false },
        fn(acc, p) { acc.union(first(p.rhs[:])) },
      )
      if not(old_set == new_set) {
        fixedpoint_reached = false
        nt_first_map[nt.num] = new_set
      }
    }
  }
  first
}
