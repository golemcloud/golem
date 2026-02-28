# MoonYacc Manual

MoonYacc is an LR(1) parser generator for MoonBit, based on Pager's LR(1) algorithm adapted from Menhir.

MoonYacc is compatible with ocamlyacc's grammar syntax (see ocamlyacc's manual [here](https://ocaml.org/manual/5.3/lexyacc.html#s:ocamlyacc-overview)), with the following differences:

- Although MoonYacc supports block comments `(* comment *)` and `/* comment */`, it is discouraged to use them in the grammar file because these comment syntaxes are not supported by MoonBit.
- It is recommended to use `%{` and `%}` to specify trailer code, instead of `%%` as in ocamlyacc.
- MoonYacc supports binding semantic variables to symbols in the grammar, which is not supported by ocamlyacc. The syntax is `var=nonterminal` or `var=TERMINAL`. Semantic variables can be used in the semantic action of the rule.
- MoonYacc supports defining terminal alias by string literal in the grammar. For example, `%token PLUS "+"` direcitve defines a terminal alias `"+"` for the terminal `PLUS`. The terminal alias can be used in the grammar rules to improve readability.
- MoonYacc does not support error recovery at the moment.

## How to use MoonYacc in a MoonBit project

There is an example repository [moonyacc-example-arithmetic](https://github.com/moonbit-community/moonyacc-example-arithmetic).

1. Add MoonYacc to the project's binary dependencies.

```bash
moon add --bin moonbitlang/yacc
```

2. Write the grammar file in the package directory where you want to generate the parser.

3. Add a pre-build rule to `moon.pkg.json` to let the Moon build system know how to generate the parser from the grammar file.

```json
{
  "pre-build": [
    {
      "command": "$mod_dir/.mooncakes/moonbitlang/yacc/moonyacc $input -o $output",
      "input": "parser.mbty",
      "output": "parser.mbt"
    }
  ]
}
```

## Using the generated parser

The generated parser typically has the following interface:

```
package parser

// Values
fn start(Array[(Token, Int, Int)]) -> Int!ParseError

// Types and methods
pub type! ParseError {
  UnexpectedToken(Token, (Int, Int), Array[TokenKind])
  UnexpectedEndOfInput(Int, Array[TokenKind])
}

pub(all) enum Token {
  ...
}
impl Token {
  kind(Self) -> TokenKind
}

pub(all) enum TokenKind {
  ...
}

// Type aliases
pub typealias Position = Int

// Traits
```

For each entry point specified by the `%start` directive, a parsing function with the same name is defined in the generated parser. There are two input modes which can be specified by a CLI option `--input-mode`: `array` and `pull`.

- In `array` mode, the parsing function has the signature `(Array[(Token, Int, Int)], initial_pos: Position? = ..) -> R!ParseError`, where the input is an array of tokens with position information and an optional initial position. If the initial position is not provided, the position of the first token will be used.
- In `pull` mode, the parsing function has the signature `(() -> (Token, Position, Position), Position) -> R!ParseError`, where the input is a token puller function and the initial position. While using `pull` mode, there is no way to indicate the end of input, so you may need to add a special token to indicate the end of input to avoid [End-of-stream conflicts](https://gallium.inria.fr/~fpottier/menhir/manual.html#sec%3Aeos). Currently, MoonYacc does not report End-of-stream conflicts in the grammar.

You can use the `%position<TypeOfPosition>` directive in the header to specify the type of position in the generated parser. For example, you can use `%position<Int>` to specify the position type as `Int`.

You can use the `%derive<Trait1, Trait2> PubTypeInGeneratedParser` directive to specify the derived traits for the public types generated in the parser. For example, you can use `%derive<Show> TokenKind` to derive the `Show` trait for the `TokenKind` enum.

## Using the command line interface

**CAUTION:** The command line interface is not stable yet.

The `moonyacc` command recognizes the following options:

- `-o` specifies the name of the output file.
- `--input-mode` specifies the input mode of the generated parser. The value can be `array` or `pull`. The default value is `array`.
- `--version` prints the version of MoonYacc.

## About the source map

By default, `moonyacc` generates a `parser.mbt.map.json` file along with the parser file. The source map is a JSON file that maps the generated parser's positions to the original grammar file's positions. The source map can be used by the Moon build system to report diagnostics with the original grammar file's positions.

It is not recommended to track the source map file in the version control system because the source map file tracks positions in UTF-8 offset, not in line and column. This may cause problems when cross-platform due to the `CR` `LF` issue.
