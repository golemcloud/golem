---
name: golem-add-moonbit-package
description: "Adds a MoonBit module and package import to a Golem project. Use when adding a mooncakes dependency or importing one of its packages."
---

# Add a MoonBit dependency

Current MoonBit projects declare module dependencies in `moon.mod` and package imports in
`moon.pkg`. The JSON files `moon.mod.json` and `moon.pkg.json` are legacy formats; do not create
them for application source.

## Add the module

Use the package manager so it selects a version and updates `moon.mod`:

```shell
moon add example/json-utils
```

The resulting module declaration has this shape:

```moonbit
name = "my-org/my-project"

import {
  "example/json-utils@0.2.0",
}
```

Use `moon add --upgrade example/json-utils` to update an existing dependency. `moon check` and
`moon build` fetch declared dependencies automatically; the old advice to run `moon install` for
project dependencies no longer applies.

## Import the package

Module dependencies make packages available, but each source package must explicitly import the
package it uses in `moon.pkg`:

```moonbit
import {
  "example/json-utils/parser" @json_parser,
}
```

Code can then call that package through `@json_parser`.

For local modules, use a `moon.work` workspace and list each module directory as a member; local
path dependencies in the new `moon.mod` format are deprecated.

Golem's stock MoonBit build template currently assumes a single-module artifact path. Adding
workspace members changes Moon's output path, so an unchanged `golem build` will not find the
component WASM. A Golem component that uses `moon.work` must override its debug and release build,
embed-source, and `componentWasm` paths. Do not present a workspace dependency as a drop-in change
to a generated Golem application.

Golem-generated MoonBit bridge modules are a deliberate exception: the bridge generator currently
emits self-contained modules with `moon.mod.json`. Do not rename or edit that generated file. Put
the application and generated module in `moon.work`, then import the generated module by its module
name from the application's `moon.mod` and import its packages from `moon.pkg`.

For an ordinary registry dependency, run `moon check` and `golem build --yes`. Confirm that the
dependency supports the component's `wasm` target; native-only and JavaScript-only packages cannot
be linked into a Golem agent.
