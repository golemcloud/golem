fn main {
  let json: Json = {
    "kind": "Expr::Try",
    "loc": {
      "file": "",
      "start": { "line": 1, "column": 2 },
      "end": { "line": 5, "column": 3 },
    },
    "children": {
      "body": {
        "kind": "Expr::Apply",
        "loc": {
          "file": "",
          "start": { "line": 1, "column": 6 },
          "end": { "line": 1, "column": 19 },
        },
        "children": {
          "func": {
            "kind": "Expr::Ident",
            "loc": {
              "file": "",
              "start": { "line": 1, "column": 6 },
              "end": { "line": 1, "column": 9 },
            },
            "children": {
              "id": {
                "kind": "Var",
                "loc": {
                  "file": "",
                  "start": { "line": 1, "column": 6 },
                  "end": { "line": 1, "column": 9 },
                },
                "children": {
                  "name": {
                    "kind": "LongIdent::Ident",
                    "loc": {
                      "file": "",
                      "start": { "line": 1, "column": 6 },
                      "end": { "line": 1, "column": 9 },
                    },
                    "children": { "value": "foo" },
                  },
                },
              },
            },
          },
          "args": {
            "kind": "Expr::Apply::ArgumentList",
            "loc": {
              "file": "",
              "start": { "line": 1, "column": 6 },
              "end": { "line": 1, "column": 19 },
            },
            "children": [
              {
                "kind": "Argument",
                "loc": {
                  "file": "",
                  "start": { "line": 1, "column": 10 },
                  "end": { "line": 1, "column": 11 },
                },
                "children": {
                  "value": {
                    "kind": "Expr::Constant",
                    "loc": {
                      "file": "",
                      "start": { "line": 1, "column": 10 },
                      "end": { "line": 1, "column": 11 },
                    },
                    "children": {
                      "constant": {
                        "kind": "Constant::Int",
                        "loc": {
                          "file": "",
                          "start": { "line": 1, "column": 10 },
                          "end": { "line": 1, "column": 11 },
                        },
                        "children": { "value": "1" },
                      },
                    },
                  },
                  "kind": {
                    "kind": "ArgumentKind::Positional",
                    "loc": {
                      "file": "",
                      "start": { "line": 1, "column": 10 },
                      "end": { "line": 1, "column": 11 },
                    },
                    "children": {},
                  },
                },
              },
              {
                "kind": "Argument",
                "loc": {
                  "file": "",
                  "start": { "line": 1, "column": 13 },
                  "end": { "line": 1, "column": 18 },
                },
                "children": {
                  "value": {
                    "kind": "Expr::Infix",
                    "loc": {
                      "file": "",
                      "start": { "line": 1, "column": 13 },
                      "end": { "line": 1, "column": 18 },
                    },
                    "children": {
                      "op": {
                        "kind": "Var",
                        "loc": {
                          "file": "",
                          "start": { "line": 1, "column": 15 },
                          "end": { "line": 1, "column": 16 },
                        },
                        "children": {
                          "name": {
                            "kind": "LongIdent::Ident",
                            "loc": {
                              "file": "",
                              "start": { "line": 1, "column": 15 },
                              "end": { "line": 1, "column": 16 },
                            },
                            "children": { "value": "*" },
                          },
                        },
                      },
                      "lhs": {
                        "kind": "Expr::Constant",
                        "loc": {
                          "file": "",
                          "start": { "line": 1, "column": 13 },
                          "end": { "line": 1, "column": 14 },
                        },
                        "children": {
                          "constant": {
                            "kind": "Constant::Int",
                            "loc": {
                              "file": "",
                              "start": { "line": 1, "column": 13 },
                              "end": { "line": 1, "column": 14 },
                            },
                            "children": { "value": "4" },
                          },
                        },
                      },
                      "rhs": {
                        "kind": "Expr::Constant",
                        "loc": {
                          "file": "",
                          "start": { "line": 1, "column": 17 },
                          "end": { "line": 1, "column": 18 },
                        },
                        "children": {
                          "constant": {
                            "kind": "Constant::Int",
                            "loc": {
                              "file": "",
                              "start": { "line": 1, "column": 17 },
                              "end": { "line": 1, "column": 18 },
                            },
                            "children": { "value": "8" },
                          },
                        },
                      },
                    },
                  },
                  "kind": {
                    "kind": "ArgumentKind::Positional",
                    "loc": {
                      "file": "",
                      "start": { "line": 1, "column": 13 },
                      "end": { "line": 1, "column": 18 },
                    },
                    "children": {},
                  },
                },
              },
            ],
          },
          "attr": {
            "kind": "ApplyAttr::NoAttr",
            "loc": {
              "file": "",
              "start": { "line": 1, "column": 6 },
              "end": { "line": 1, "column": 19 },
            },
            "children": {},
          },
        },
      },
      "catch": {
        "kind": "Expr::Try::CaseList",
        "loc": {
          "file": "",
          "start": { "line": 1, "column": 2 },
          "end": { "line": 5, "column": 3 },
        },
        "children": [
          {
            "kind": "Case",
            "loc": {
              "file": "",
              "start": { "line": 2, "column": 4 },
              "end": { "line": 2, "column": 8 },
            },
            "children": {
              "pattern": {
                "kind": "Pattern::Var",
                "loc": {
                  "file": "",
                  "start": { "line": 2, "column": 4 },
                  "end": { "line": 2, "column": 5 },
                },
                "children": {
                  "value": {
                    "kind": "Binder",
                    "loc": {
                      "file": "",
                      "start": { "line": 2, "column": 4 },
                      "end": { "line": 2, "column": 5 },
                    },
                    "children": { "name": "e" },
                  },
                },
              },
              "guard": null,
              "body": {
                "kind": "Expr::Group",
                "loc": {
                  "file": "",
                  "start": { "line": 2, "column": 9 },
                  "end": { "line": 2, "column": 8 },
                },
                "children": {
                  "expr": {
                    "kind": "Expr::Apply",
                    "loc": {
                      "file": "",
                      "start": { "line": 3, "column": 6 },
                      "end": { "line": 3, "column": 14 },
                    },
                    "children": {
                      "func": {
                        "kind": "Expr::Ident",
                        "loc": {
                          "file": "",
                          "start": { "line": 3, "column": 6 },
                          "end": { "line": 3, "column": 11 },
                        },
                        "children": {
                          "id": {
                            "kind": "Var",
                            "loc": {
                              "file": "",
                              "start": { "line": 3, "column": 6 },
                              "end": { "line": 3, "column": 11 },
                            },
                            "children": {
                              "name": {
                                "kind": "LongIdent::Ident",
                                "loc": {
                                  "file": "",
                                  "start": { "line": 3, "column": 6 },
                                  "end": { "line": 3, "column": 11 },
                                },
                                "children": { "value": "print" },
                              },
                            },
                          },
                        },
                      },
                      "args": {
                        "kind": "Expr::Apply::ArgumentList",
                        "loc": {
                          "file": "",
                          "start": { "line": 3, "column": 6 },
                          "end": { "line": 3, "column": 14 },
                        },
                        "children": [
                          {
                            "kind": "Argument",
                            "loc": {
                              "file": "",
                              "start": { "line": 3, "column": 12 },
                              "end": { "line": 3, "column": 13 },
                            },
                            "children": {
                              "value": {
                                "kind": "Expr::Ident",
                                "loc": {
                                  "file": "",
                                  "start": { "line": 3, "column": 12 },
                                  "end": { "line": 3, "column": 13 },
                                },
                                "children": {
                                  "id": {
                                    "kind": "Var",
                                    "loc": {
                                      "file": "",
                                      "start": { "line": 3, "column": 12 },
                                      "end": { "line": 3, "column": 13 },
                                    },
                                    "children": {
                                      "name": {
                                        "kind": "LongIdent::Ident",
                                        "loc": {
                                          "file": "",
                                          "start": { "line": 3, "column": 12 },
                                          "end": { "line": 3, "column": 13 },
                                        },
                                        "children": { "value": "e" },
                                      },
                                    },
                                  },
                                },
                              },
                              "kind": {
                                "kind": "ArgumentKind::Positional",
                                "loc": {
                                  "file": "",
                                  "start": { "line": 3, "column": 12 },
                                  "end": { "line": 3, "column": 13 },
                                },
                                "children": {},
                              },
                            },
                          },
                        ],
                      },
                      "attr": {
                        "kind": "ApplyAttr::NoAttr",
                        "loc": {
                          "file": "",
                          "start": { "line": 3, "column": 6 },
                          "end": { "line": 3, "column": 14 },
                        },
                        "children": {},
                      },
                    },
                  },
                  "group": {
                    "kind": "Group::Brace",
                    "loc": {
                      "file": "",
                      "start": { "line": 2, "column": 9 },
                      "end": { "line": 2, "column": 8 },
                    },
                    "children": {},
                  },
                },
              },
            },
          },
        ],
      },
      "catch_all": false,
      "try_else": null,
      "has_try": true,
    },
  }
  println(json)
}
