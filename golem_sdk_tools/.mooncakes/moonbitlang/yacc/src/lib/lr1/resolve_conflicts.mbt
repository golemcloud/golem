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
pub(all) enum ResolveConflictError {
  Reduce_conflict_resolved_by_presentation_order(
    LR1State,
    Lookahead,
    Array[@grm.Production],
    @grm.Production
  )
  Shift_reduce_conflict_resolved_without_precedence(
    LR1State,
    Lookahead,
    LR1State,
    @grm.Production
  )
  Shift_reduce_conflict_not_resolved_because_of_non_assoc(
    LR1State,
    Lookahead,
    LR1State,
    @grm.Production
  )
}

///|
/// Resolve conflicts by using precedence and associativity information
pub fn resolve_conflicts(
  conflicts : @hashmap2.T[ConflictLocation, @list.List[Decision]],
) -> Array[ResolveConflictError] {
  let errors = []
  for location, decisions in conflicts {
    let { state, input } = location
    let shift = []
    let reduce = []
    for decision in decisions {
      match decision {
        Shift(state) => shift.push((input, state))
        Reduce(production) => reduce.push(production)
        Accept => panic()
        Conflict(_) => panic()
      }
    }
    guard shift.length() <= 1
    guard reduce.length() >= 1
    reduce.sort_by_key(fn(prod) {
      // production presentation order in grammar
      prod.num
    })
    let best_reduce = reduce[0]
    let decision = match shift {
      [] => Ok(Reduce(best_reduce))
      [(input, shift_state)] => {
        let term_prec = match input {
          Input(term) => term.prec
          EndOfInput => None
        }
        match (term_prec, best_reduce.prec) {
          (None, None) => {
            errors.push(
              Shift_reduce_conflict_resolved_without_precedence(
                state, input, shift_state, best_reduce,
              ),
            )
            Ok(Shift(shift_state))
          }
          (None, Some(_)) => Ok(Reduce(best_reduce))
          (Some(_), None) => Ok(Shift(shift_state))
          (Some((prec1, assoc1)), Some(prec2)) =>
            if prec1 > prec2 {
              Ok(Shift(shift_state))
            } else if prec2 > prec1 {
              Ok(Reduce(best_reduce))
            } else {
              match assoc1 {
                LeftAssoc => Ok(Reduce(best_reduce))
                RightAssoc => Ok(Shift(shift_state))
                NonAssoc =>
                  Err(
                    Shift_reduce_conflict_not_resolved_because_of_non_assoc(
                      state, input, shift_state, best_reduce,
                    ),
                  )
              }
            }
        }
      }
      _ => panic()
    }
    if reduce.length() >= 2 &&
      decision is Ok(Reduce(prod)) &&
      prod.num == best_reduce.num {
      errors.push(
        Reduce_conflict_resolved_by_presentation_order(
          state, input, reduce, best_reduce,
        ),
      )
    }
    match decision {
      Ok(decision) => state.action[input] = decision
      Err(error) => errors.push(error)
    }
  }
  errors
}
