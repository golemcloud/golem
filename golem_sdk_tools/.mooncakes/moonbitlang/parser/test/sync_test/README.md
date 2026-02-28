# Imported Tests

The tests in this package are copied from Moonbit's OCaml implementation, to ensure consistency between the two implementations.

We synchronize them weekly. Please **do not edit or add new tests here**.

## How to synchronize

1. Run script `script/export_parse_test.js` in ocaml implementation, export the tests to `__snapshot__`.
2. Run `moon run test/sync_test/generator/generator.mbt` to generate the test runner, normize the json output.
3. Run `moon test` and review the changes.

