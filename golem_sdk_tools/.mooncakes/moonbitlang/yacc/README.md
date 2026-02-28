# MoonYacc

https://moonyacc-playground.netlify.app/

MoonYacc is a LR(1) parser generator for MoonBit programming language.

See [calc.mbty](https://github.com/moonbitlang/moonyacc/blob/master/src/tests/calc_test/calc.mbty) for an example of grammar.

See [MANUAL.md](https://github.com/moonbitlang/moonyacc/blob/master/doc/MANUAL.md) for more details.

## Features

- Compatible with ocamlyacc's syntax

## Status

This software is in alpha stage. Though it may have some issues, it is already usable for most purposes.

## Acknowledgements

This project, moonyacc, incorporates and adapts two key algorithms from Menhir, a powerful parser generator:

### LR(0) Closure Algorithm:

The LR(0) closure algorithm is used to compute the closure of a set of LR(0) items, which is a fundamental step in constructing the LR(0) automaton. This algorithm has been adapted and implemented in moonyacc to support the generation of LR(0) automaton.

### LR(1) Pager's Algorithm:

The LR(1) Pager's algorithm is used to efficiently compute the LR(1) states and their transitions. It is a critical component for building LR(1) parsers with reduced state space. This algorithm has been adapted to enhance the parsing capabilities of moonyacc.

Both algorithms have been reimplemented and integrated into moonyacc to provide robust and efficient parsing functionality. While the core ideas are inspired by Menhir, the implementation has been tailored to fit the architecture and goals of this project.
