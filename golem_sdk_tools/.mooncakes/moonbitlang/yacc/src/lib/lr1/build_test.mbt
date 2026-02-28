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
fn test_grammar(
  grammar~ : @grm.Grammar,
  filename~ : String,
  it : @test.Test,
) -> Unit raise {
  it.write("## grammar\n")
  it.write("### terminals\n")
  for terminal in grammar.terminals {
    it.write("- `")
    it.write(terminal)
    it.write("`\n")
  }
  it.write("### start\n")
  it.write("- `")
  it.write(grammar.starts[0].lhs)
  it.write("`\n")
  it.write("### productions\n")
  for nt in grammar.nonterminals {
    for production in nt.productions {
      it.write("- `")
      it.write(production.num)
      it.write(", ")
      it.write(production)
      it.write("`\n")
    }
  }
  let automaton = @lr1.Automaton::build(grammar, user_eoi=false)
  it.write("## states\n")
  for state in automaton.states {
    it.write("### state ")
    it.write(state.num)
    it.write("\n")
    it.write("#### items\n")
    for item_group in state.iter_item_groups() {
      it.write("- `")
      it.write(item_group)
      it.write("`\n")
    }
  }
  it.write("## table\n")
  it.write("|state")
  for terminal in grammar.terminals {
    it.write("|")
    it.write(terminal)
  }
  it.write("|$")
  for nonterminal in grammar.nonterminals {
    if nonterminal != grammar.starts[0].lhs {
      it.write("|")
      it.write(nonterminal)
    }
  }
  it.write("|\n")
  it.write("|")
  for _ in grammar.terminals {
    it.write("-|")
  }
  it.write("-|")
  for nonterminal in grammar.nonterminals {
    if nonterminal != grammar.starts[0].lhs {
      it.write("-|")
    }
  }
  it.write("-|\n")
  for state in automaton.states {
    it.write("|")
    it.write(state.num)
    for terminal in grammar.terminals {
      it.write("|")
      match state.action.get(Input(terminal)) {
        Some(Shift(next_state)) => {
          it.write("s")
          it.write(next_state.num)
        }
        Some(Reduce(production)) => {
          it.write("r")
          it.write(production.num)
        }
        Some(Accept) =>
          // This should never happen
          panic()
        Some(Conflict(_)) => it.write("c")
        None => ()
      }
    }
    match state.action.get(EndOfInput) {
      Some(Accept) => it.write("|acc")
      Some(Reduce(production)) => {
        it.write("|r")
        it.write(production.num)
      }
      _ => it.write("|")
    }
    for nonterminal in grammar.nonterminals {
      if nonterminal != grammar.starts[0].lhs {
        it.write("|")
        match state.goto.get(NT(nonterminal)) {
          Some(next_state) => it.write(next_state.num)
          None => ()
        }
      }
    }
    it.write("|\n")
  }
  it.snapshot(filename~)
}

///|
test "1" (it : @test.Test) {
  // test data from https://en.wikipedia.org/wiki/LR_parser
  let grammar = @grm.build(fn(ctx) {
    let _ = ctx.add_production("S", ["E"])
    let _ = ctx.add_production("E", ["E", "*", "B"])
    let _ = ctx.add_production("E", ["E", "+", "B"])
    let _ = ctx.add_production("E", ["B"])
    let _ = ctx.add_production("B", ["0"])
    let _ = ctx.add_production("B", ["1"])
    ctx.set_start("S")
  })
  test_grammar(grammar~, filename="1.md", it)
}

///|
test "2" (it : @test.Test) {
  // test data from https://www.javatpoint.com/clr-1-parsing
  let grammar = @grm.build(fn(ctx) {
    let _ = ctx.add_production("S'", ["S"])
    let _ = ctx.add_production("S", ["A", "A"])
    let _ = ctx.add_production("A", ["a", "A"])
    let _ = ctx.add_production("A", ["b"])
    ctx.set_start("S'")
  })
  test_grammar(grammar~, filename="2.md", it)
}

///|
test "3" (it : @test.Test) {
  // test data from https://www.javatpoint.com/clr-1-parsing
  let grammar = @grm.build(fn(ctx) {
    let _ = ctx.add_production("Start", ["Add"])
    let _ = ctx.add_production("Add", ["Add", "+", "Factor"])
    let _ = ctx.add_production("Add", ["Add", "-", "Factor"])
    let _ = ctx.add_production("Add", ["Factor"])
    let _ = ctx.add_production("Factor", ["Factor", "*", "Term"])
    let _ = ctx.add_production("Factor", ["Term"])
    let _ = ctx.add_production("Term", ["(", "Add", ")"])
    let _ = ctx.add_production("Term", ["Number"])
    ctx.set_start("Start")
  })
  test_grammar(grammar~, filename="3.md", it)
}
