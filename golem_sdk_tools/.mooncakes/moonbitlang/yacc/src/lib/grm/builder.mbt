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
pub type BuilderSymbol = String

///|
pub struct BuilderContext {
  priv _set_prec : (BuilderSymbol, Associativity, Int) -> Unit
  priv _add_production : (BuilderSymbol, Array[BuilderSymbol]) -> Int
  priv _set_start : (BuilderSymbol) -> Unit
}

///|
pub fn BuilderContext::set_prec(
  self : BuilderContext,
  term : BuilderSymbol,
  assoc : Associativity,
  level : Int,
) -> Unit {
  (self._set_prec)(term, assoc, level)
}

///|
pub fn BuilderContext::add_production(
  self : BuilderContext,
  lhs : BuilderSymbol,
  rhs : Array[BuilderSymbol],
) -> Int {
  (self._add_production)(lhs, rhs)
}

///|
pub fn BuilderContext::set_start(
  self : BuilderContext,
  name : BuilderSymbol,
) -> Unit {
  (self._set_start)(name)
}

///|
pub fn build(f : (BuilderContext) -> Unit) -> Grammar {
  let terminal_by_name = @sorted_map.new()
  let nonterminal_by_name = @sorted_map.new()
  let terminals = []
  let nonterminals = []
  let productions = []
  fn get_terminal(name : BuilderSymbol) -> Terminal {
    match terminal_by_name.get(name) {
      Some(t) => t
      None => {
        let t = Terminal::{ num: terminal_by_name.length(), name, prec: None }
        terminal_by_name[name] = t
        terminals.push(t)
        t
      }
    }
  }

  fn get_nonterminal(name : BuilderSymbol) -> Nonterminal {
    match nonterminal_by_name.get(name) {
      Some(nt) => nt
      None => {
        let nt = Nonterminal::{
          num: nonterminal_by_name.length(),
          name,
          productions: [],
        }
        nonterminal_by_name[name] = nt
        nonterminals.push(nt)
        nt
      }
    }
  }

  let starts : Array[String] = []
  let mut num_productions = 0
  let mut production_added = false
  let defer_ = []
  let ctx = BuilderContext::{
    _add_production: fn(lhs, rhs) {
      let nt = get_nonterminal(lhs)
      let num = num_productions
      num_productions += 1
      defer_.push(fn() {
        let mut last_prec = None
        let production = Production::{
          num,
          lhs: nt,
          rhs: rhs.map(fn(symbol) {
            match nonterminal_by_name.get(symbol) {
              Some(nt) => NT(nt)
              None => T(get_terminal(symbol))
            }
          }),
          prec: None,
        }
        productions.push(production)
        for symbol in production.rhs {
          match symbol {
            T(t) => last_prec = t.prec.map(it => it.0)
            NT(_) => ()
          }
        }
        production.prec = last_prec
        nt.productions.push(production)
      })
      production_added = true
      num
    },
    _set_prec: fn(term, assoc, level) {
      guard not(production_added)
      let term = get_terminal(term)
      term.prec = Some((level, assoc))
    },
    _set_start: fn(name) { starts.push(name) },
  }
  f(ctx)
  for f in defer_ {
    f()
  }
  let starts = starts.map(fn(name) {
    let start_nt = get_nonterminal(name)
    guard start_nt.productions.length() == 1 &&
      start_nt.productions[0].rhs.length() == 1
    start_nt.productions[0]
  })
  { starts, terminals, nonterminals, productions }
}
