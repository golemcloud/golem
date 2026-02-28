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

// About precedence and associativity:
//
// Each terminal may be assigned a precedence and associativity. Each rule is
// then assigned the precedence of its rightmost terminal. If a shift/reduce
// conflict occurs, the conflict is resolved silently if the terminal and the
// rule in the conflict have precedences. If the terminal has the higher
// precedence, the shift is chosen. If the rule has the higher precedence, the
// reduction is chosen. If both the terminal and the rule have the same
// precedence, then the associativity of the terminal is used to resolve the
// conflict. If the terminal is left associative, the reduction is chosen. If
// the terminal is right associative, the shift is chosen. Terminals may be
// declared to be nonassociative, also, in which case an error message is
// produced if the associativity is need to resolve the parsing conflict.
//
// If a terminal or a rule in a shift/reduce conflict does not have a
// precedence, then an error message is produced and the shift is chosen.
//
// In reduce/reduce conflicts, an error message is always produced and the first
// rule listed in the specification is chosen for reduction

///|
pub(all) enum Associativity {
  LeftAssoc
  RightAssoc
  NonAssoc
} derive(Eq)

///|
pub(all) struct Terminal {
  mut num : Int
  name : String
  mut prec : (Int, Associativity)?
}

///|
pub impl Eq for Terminal with equal(self, other) {
  self.num == other.num
}

///|
pub impl Compare for Terminal with compare(self, other) {
  Int::compare(self.num, other.num)
}

///|
pub impl Hash for Terminal with hash_combine(self, hasher) {
  hasher.combine_int(self.num)
}

///|
pub impl Show for Terminal with output(self, logger) -> Unit {
  logger.write_string(self.name)
}

///|
pub(all) enum Nullability {
  Null // Alwasy produces epsilon
  Nullable // May produce epsilon
  NonNullable // Never produces epsilon
} derive(Eq)

///|
pub(all) struct Nonterminal {
  mut num : Int
  name : String
  productions : Array[Production]
}

///|
pub impl Eq for Nonterminal with equal(self, other) {
  self.num == other.num
}

///|
pub impl Compare for Nonterminal with compare(self, other) {
  Int::compare(self.num, other.num)
}

///|
pub impl Hash for Nonterminal with hash_combine(self, hasher) {
  hasher.combine_int(self.num)
}

///|
pub impl Show for Nonterminal with output(self, logger) {
  logger.write_string(self.name)
}

///|
pub(all) enum Symbol {
  T(Terminal)
  NT(Nonterminal)
} derive(Eq, Compare, Hash)

///|
pub impl Show for Symbol with output(self, logger) -> Unit {
  match self {
    Symbol::T(t) => logger.write_object(t)
    Symbol::NT(nt) => logger.write_object(nt)
  }
}

///|
pub(all) struct Production {
  mut num : Int
  lhs : Nonterminal
  rhs : Array[Symbol]
  mut prec : Int?
}

///|
pub impl Eq for Production with equal(self, other) {
  self.num == other.num
}

///|
pub impl Compare for Production with compare(self, other) {
  Int::compare(self.num, other.num)
}

///|
pub impl Hash for Production with hash_combine(self, hasher) {
  hasher.combine_int(self.num)
}

///|
pub fn[Logger : Logger] Production::output_with_opt_dot(
  self : Production,
  logger : Logger,
  dot? : Int,
) -> Unit {
  (logger : &Logger).write_object(self.lhs)
  logger.write_string(" →")
  for i, symbol in self.rhs {
    match dot {
      Some(d) => if i == d { logger.write_string(" •") }
      None => ()
    }
    logger.write_string(" ")
    (logger : &Logger).write_object(symbol)
  }
  match dot {
    Some(d) => if d == self.rhs.length() { logger.write_string(" •") }
    None => ()
  }
}

///|
pub impl Show for Production with output(self, logger) -> Unit {
  self.output_with_opt_dot(logger)
}

///|
pub(all) struct Grammar {
  starts : Array[Production]
  terminals : Array[Terminal]
  nonterminals : Array[Nonterminal]
  productions : Array[Production]
}
