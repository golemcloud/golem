#!/usr/bin/env python3
"""Generate tables.rs from jq 1.8.2's bison-generated parser.

    gen_tables.py JQ_SRC_DIR > tables.rs

JQ_SRC_DIR is the `src` directory of the jq 1.8.2 release tarball
(https://github.com/jqlang/jq/releases/download/jq-1.8.2/jq-1.8.2.tar.gz, SHA-256
71b8d6e8f5fe81f6c6d0d110e3892251f6ce76ed095abd315e26e6e1193af3af), whose `parser.c` was made by
GNU Bison 3.8.2 from `parser.y`. The tables are copied as they are; the rules whose actions report
an error are found by the comments bison writes above each action.
"""
import json
import pathlib
import re
import sys

src = pathlib.Path(sys.argv[1])
parser_c = (src / "parser.c").read_text()
parser_h = (src / "parser.h").read_text()

assert "A Bison parser, made by GNU Bison 3.8.2." in parser_c


def define(name):
    return int(re.search(rf"#define {name}\s+\(?(-?\d+)\)?", parser_c).group(1))


def array(name):
    body = re.search(rf"static const \w+ {name}\[\] =\s*\{{(.*?)\}};", parser_c, re.S).group(1)
    return [int(x) for x in body.replace("\n", " ").split(",") if x.strip()]


def names():
    body = re.search(r"static const char \*const yytname\[\] =\s*\{(.*?)\};", parser_c, re.S).group(1)
    return re.findall(r'"((?:[^"\\]|\\.)*)"', body.replace("YY_NULLPTR", ""))


tokens = dict(
    (m.group(1), int(m.group(2)))
    for m in re.finditer(r"^#define (\w+) (-?\d+)$", parser_h, re.M)
    # The lexer never returns bison's own tokens nor the precedence-only FUNCDEF and NONOPT.
    if m.group(1) not in ("YYEMPTY", "YYEOF", "YYerror", "YYUNDEF", "FUNCDEF", "NONOPT")
)


def rule(comment):
    match = re.search(rf"  case (\d+): /\* {re.escape(comment)}  \*/", parser_c)
    assert match, comment
    return int(match.group(1))


rules = {
    "TOP_LEVEL_QUERY": "TopLevel: Module Imports Query",
    "MODULE_META": 'Module: "module" Query \';\'',
    "IMPORT_META": "Import: ImportWhat Query ';'",
    "IMPORT_FROM": "ImportFrom: String",
    "BREAK_ERROR": 'Term: "break" error',
    "DOT_ERROR": "Term: '.' error",
    "DOT_IDENT_ERROR": "Term: '.' IDENT error",
    "IF_ERROR": 'Term: "if" Query "then" error',
    "TRY_ERROR": 'Term: "try" Expr "catch" error',
    "OBJ_PAT_KEY": "ObjPat: '(' Query ')' ':' Pattern",
    "OBJ_PAT_ERROR": "ObjPat: error ':' Pattern",
    "DICT_PAIR_KEY": "DictPair: '(' Query ')' ':' DictExpr",
    "DICT_PAIR_ERROR": "DictPair: error ':' DictExpr",
}


# How each rule's right-hand side nests, for the depth jaq's evaluator reaches: "N" for a symbol
# one level deeper than the rule (the query in parentheses, an `if`'s branches, a string
# interpolation, ...), "C" for the rest of a flat chain (the next `|` or `,` stage, the next
# operand of a binary operator, the next path part, ...), which jaq parses, compiles and runs
# without nesting, "-" for a symbol at the rule's own level. Rules not listed are all "-".
NESTING = {
    'Module: "module" Query \';\'': "-N-",
    "FuncDefs: FuncDef FuncDefs": "-C",
    "Query: FuncDef Query": "-C",
    'Query: Expr "as" Patterns \'|\' Query': "----C",
    'Query: "label" BINDING \'|\' Query': "---N",
    "Query: Query '|' Query": "C-C",
    "Query: Query ',' Query": "C-C",
    "Import: ImportWhat Query ';'": "-N-",
    "FuncDef: \"def\" IDENT ':' Query ';'": "---N-",
    "FuncDef: \"def\" IDENT '(' Params ')' ':' Query ';'": "------N-",
    "Params: Params ';' Param": "C--",
    "QQString: QQString QQSTRING_TEXT": "C-",
    "QQString: QQString QQSTRING_INTERP_START Query QQSTRING_INTERP_END": "C-N-",
    'ElseBody: "elif" Query "then" Query ElseBody': "-N-NC",
    'ElseBody: "else" Query "end"': "-N-",
    "Term: Term FIELD '?'": "C--",
    "Term: Term '.' String '?'": "C---",
    "Term: Term FIELD": "C-",
    "Term: Term '.' String": "C--",
    "Term: Term '[' Query ']' '?'": "C-N--",
    "Term: Term '[' Query ']'": "C-N-",
    "Term: Term '.' '[' Query ']' '?'": "C--N--",
    "Term: Term '.' '[' Query ']'": "C--N-",
    "Term: Term '[' ']' '?'": "C---",
    "Term: Term '[' ']'": "C--",
    "Term: Term '.' '[' ']' '?'": "C----",
    "Term: Term '.' '[' ']'": "C---",
    "Term: Term '[' Query ':' Query ']' '?'": "C-N-N--",
    "Term: Term '[' Query ':' ']' '?'": "C-N---",
    "Term: Term '[' ':' Query ']' '?'": "C--N--",
    "Term: Term '[' Query ':' Query ']'": "C-N-N-",
    "Term: Term '[' Query ':' ']'": "C-N--",
    "Term: Term '[' ':' Query ']'": "C--N-",
    "Term: Term '?'": "C-",
    "Term: '-' Term": "-N",
    "Term: '(' Query ')'": "-N-",
    "Term: '[' Query ']'": "-N-",
    "Term: '{' DictPairs '}'": "-N-",
    'Term: "reduce" Expr "as" Patterns \'(\' Query \';\' Query \')\'': "-N-N-N-N-",
    'Term: "foreach" Expr "as" Patterns \'(\' Query \';\' Query \';\' Query \')\'': "-N-N-N-N-N-",
    'Term: "foreach" Expr "as" Patterns \'(\' Query \';\' Query \')\'': "-N-N-N-N-",
    'Term: "if" Query "then" Query ElseBody': "-N-N-",
    'Term: "try" Expr "catch" Expr': "-N-N",
    'Term: "try" Expr': "-N",
    "Term: IDENT '(' Args ')'": "--N-",
    "Args: Args ';' Arg": "C--",
    # each `?//` alternative is tried inside the one before
    'RepPatterns: RepPatterns "?//" Pattern': "N--",
    'Patterns: RepPatterns "?//" Pattern': "N--",
    "Pattern: '[' ArrayPats ']'": "-N-",
    "Pattern: '{' ObjPats '}'": "-N-",
    "ArrayPats: ArrayPats ',' Pattern": "C--",
    "ObjPats: ObjPats ',' ObjPat": "C--",
    "ObjPat: '(' Query ')' ':' Pattern": "-N---",
    "DictPairs: DictPair ',' DictPairs": "--C",
    "DictPair: '(' Query ')' ':' DictExpr": "-N---",
    "DictExpr: DictExpr '|' DictExpr": "C-C",
}
for op in ["\"//\"", "'='", '"or"', '"and"', '"//="', '"|="', "'+'", '"+="', "'-'", '"-="', "'*'", '"*="',
           "'/'", "'%'", '"/="', '"%="', '"=="', '"!="', "'<'", "'>'", '"<="', '">="']:
    NESTING[f"Expr: Expr {op} Expr"] = "C-C"


def nesting_masks():
    comments = dict(
        (int(m.group(1)), m.group(2))
        for m in re.finditer(r"  case (\d+): /\* (.*?)  \*/", parser_c)
    )
    lengths = array("yyr2")
    nest = [0] * len(lengths)
    seen = set()
    for number, text in comments.items():
        spec = NESTING.get(text)
        if spec is None:
            continue
        seen.add(text)
        assert len(spec) == lengths[number], (text, spec)
        for position, mark in enumerate(spec):
            if mark == "N":
                nest[number] |= 1 << position
    assert seen == set(NESTING), set(NESTING) - seen
    return nest


def emit_array(name, rust_type, values):
    print(f"pub(super) const {name}: [{rust_type}; {len(values)}] = [")
    line = "   "
    for value in values:
        item = f" {value},"
        if len(line) + len(item) > 100:
            print(line)
            line = "   "
        line += item
    print(line)
    print("];")
    print()


ntokens = define("YYNTOKENS")
print("// Generated by gen_tables.py from jq 1.8.2's src/parser.c (GNU Bison 3.8.2); do not edit.")
print("//")
print("// jq's license (MIT), from its COPYING:")
print("//")
license_text = (src.parent / "COPYING").read_text().split("\n\n\n")[0].strip()
for line in license_text.splitlines():
    print(f"// {line}".rstrip())
print()
print(f"pub(super) const YYFINAL: usize = {define('YYFINAL')};")
print(f"pub(super) const YYLAST: i32 = {define('YYLAST')};")
print(f"pub(super) const YYNTOKENS: i32 = {ntokens};")
print(f"pub(super) const YYMAXUTOK: i32 = {define('YYMAXUTOK')};")
print(f"pub(super) const YYPACT_NINF: i32 = {define('YYPACT_NINF')};")
print(f"pub(super) const YYTABLE_NINF: i32 = {define('YYTABLE_NINF')};")
print()
print("/// Token codes, as the lexer returns them (`parser.h`); a character is its own code.")
for name, value in tokens.items():
    print(f"pub(super) const {name}: i32 = {value};")
print()
print("/// Rules whose actions report an error, and the one that makes a program with a main query.")
for name, comment in rules.items():
    print(f"pub(super) const RULE_{name}: usize = {rule(comment)};")
print()
emit_array("YYTRANSLATE", "u8", array("yytranslate"))
emit_array("YYPACT", "i16", array("yypact"))
emit_array("YYDEFACT", "u8", array("yydefact"))
emit_array("YYPGOTO", "i16", array("yypgoto"))
emit_array("YYDEFGOTO", "u8", array("yydefgoto"))
emit_array("YYTABLE", "i16", array("yytable"))
emit_array("YYCHECK", "i16", array("yycheck"))
emit_array("YYR1", "u8", array("yyr1"))
emit_array("YYR2", "u8", array("yyr2"))
print("/// Per rule, the right-hand-side positions one level deeper than the rule (bit `i` for")
print("/// symbol `i`).")
emit_array("YYNEST", "u16", nesting_masks())
token_names = [n.replace("\\\"", "\"").replace("\\\\", "\\") for n in names()[:ntokens]]
print("/// The terminals' names, as bison spells them (`yytname`).")
print(f"pub(super) const YYTNAME: [&str; {ntokens}] = [")
for name in token_names:
    print(f"    {json.dumps(name)},")
print("];")
