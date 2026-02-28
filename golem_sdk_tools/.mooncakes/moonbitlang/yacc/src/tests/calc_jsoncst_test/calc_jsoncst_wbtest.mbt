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
test {
  let tokens = [
    NUMBER,
    PLUS,
    LPAREN,
    NUMBER,
    MINUS,
    NUMBER,
    RPAREN,
    STAR,
    NUMBER,
  ]
  let mut cursor = 0
  fn read_token() -> (Token, Int, Int) {
    guard cursor < tokens.length() else { (EOF, cursor, cursor) }
    let token = tokens[cursor]
    cursor += 1
    (token, cursor - 1, cursor)
  }

  let result = start(read_token, 0)
  @json.inspect(result, content={
    "type": "NONTERMINAL",
    "name": "start",
    "rule_index": 0,
    "clause_index": 0,
    "children": [
      {
        "type": "NONTERMINAL",
        "name": "add",
        "rule_index": 1,
        "clause_index": 0,
        "children": [
          {
            "type": "NONTERMINAL",
            "name": "add",
            "rule_index": 1,
            "clause_index": 2,
            "children": [
              {
                "type": "NONTERMINAL",
                "name": "factor",
                "rule_index": 2,
                "clause_index": 1,
                "children": [
                  {
                    "type": "NONTERMINAL",
                    "name": "term",
                    "rule_index": 3,
                    "clause_index": 0,
                    "children": [
                      {
                        "type": "TERMINAL",
                        "name": "NUMBER",
                        "data": Json::null(),
                        "start": 0,
                        "end": 1,
                      },
                    ],
                    "start": 0,
                    "end": 1,
                  },
                ],
                "start": 0,
                "end": 1,
              },
            ],
            "start": 0,
            "end": 1,
          },
          {
            "type": "TERMINAL",
            "name": "PLUS",
            "data": Json::null(),
            "start": 1,
            "end": 2,
          },
          {
            "type": "NONTERMINAL",
            "name": "factor",
            "rule_index": 2,
            "clause_index": 0,
            "children": [
              {
                "type": "NONTERMINAL",
                "name": "factor",
                "rule_index": 2,
                "clause_index": 1,
                "children": [
                  {
                    "type": "NONTERMINAL",
                    "name": "term",
                    "rule_index": 3,
                    "clause_index": 1,
                    "children": [
                      {
                        "type": "TERMINAL",
                        "name": "LPAREN",
                        "data": Json::null(),
                        "start": 2,
                        "end": 3,
                      },
                      {
                        "type": "NONTERMINAL",
                        "name": "add",
                        "rule_index": 1,
                        "clause_index": 1,
                        "children": [
                          {
                            "type": "NONTERMINAL",
                            "name": "add",
                            "rule_index": 1,
                            "clause_index": 2,
                            "children": [
                              {
                                "type": "NONTERMINAL",
                                "name": "factor",
                                "rule_index": 2,
                                "clause_index": 1,
                                "children": [
                                  {
                                    "type": "NONTERMINAL",
                                    "name": "term",
                                    "rule_index": 3,
                                    "clause_index": 0,
                                    "children": [
                                      {
                                        "type": "TERMINAL",
                                        "name": "NUMBER",
                                        "data": Json::null(),
                                        "start": 3,
                                        "end": 4,
                                      },
                                    ],
                                    "start": 3,
                                    "end": 4,
                                  },
                                ],
                                "start": 3,
                                "end": 4,
                              },
                            ],
                            "start": 3,
                            "end": 4,
                          },
                          {
                            "type": "TERMINAL",
                            "name": "MINUS",
                            "data": Json::null(),
                            "start": 4,
                            "end": 5,
                          },
                          {
                            "type": "NONTERMINAL",
                            "name": "factor",
                            "rule_index": 2,
                            "clause_index": 1,
                            "children": [
                              {
                                "type": "NONTERMINAL",
                                "name": "term",
                                "rule_index": 3,
                                "clause_index": 0,
                                "children": [
                                  {
                                    "type": "TERMINAL",
                                    "name": "NUMBER",
                                    "data": Json::null(),
                                    "start": 5,
                                    "end": 6,
                                  },
                                ],
                                "start": 5,
                                "end": 6,
                              },
                            ],
                            "start": 5,
                            "end": 6,
                          },
                        ],
                        "start": 3,
                        "end": 6,
                      },
                      {
                        "type": "TERMINAL",
                        "name": "RPAREN",
                        "data": Json::null(),
                        "start": 6,
                        "end": 7,
                      },
                    ],
                    "start": 2,
                    "end": 7,
                  },
                ],
                "start": 2,
                "end": 7,
              },
              {
                "type": "TERMINAL",
                "name": "STAR",
                "data": Json::null(),
                "start": 7,
                "end": 8,
              },
              {
                "type": "NONTERMINAL",
                "name": "term",
                "rule_index": 3,
                "clause_index": 0,
                "children": [
                  {
                    "type": "TERMINAL",
                    "name": "NUMBER",
                    "data": Json::null(),
                    "start": 8,
                    "end": 9,
                  },
                ],
                "start": 8,
                "end": 9,
              },
            ],
            "start": 2,
            "end": 9,
          },
        ],
        "start": 0,
        "end": 9,
      },
      {
        "type": "TERMINAL",
        "name": "EOF",
        "data": Json::null(),
        "start": 9,
        "end": 9,
      },
    ],
    "start": 0,
    "end": 9,
  })
}
