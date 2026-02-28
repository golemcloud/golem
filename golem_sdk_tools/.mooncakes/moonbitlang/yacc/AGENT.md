To avoid cyclic dependencies, we use a bootstrap parser to build the main parser. This is necessary because the main parser depends on the bootstrap parser, and we need to ensure that the bootstrap parser is built first.

The tests also depend on the bootstrap parser, so we need to build it before running the tests.

The built bootstrap parser is placed in the `boot` directory, and it is used to build the main parser.

## Build the bootstrap parser

```
moon clean
make boot
```
