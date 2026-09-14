---
name: golem-add-moonbit-package
description: "Adding MoonBit package dependencies to a Golem project. Use when the user asks to add a mooncakes dependency, library, or package to a MoonBit project."
---

# Adding a MoonBit Package Dependency

## Overview

MoonBit projects manage dependencies through the `moon.mod.json` file at the project root. Dependencies are published on [mooncakes.io](https://mooncakes.io) and installed with the `moon` CLI.

## Steps

1. **Edit `moon.mod.json`** — add the package to the `"deps"` section
2. **Run `moon install`** — download and install the dependency
3. **Use the package** — import it in your `.mbt` files

## Adding a Dependency

Edit `moon.mod.json` and add the package under the `"deps"` object with a version constraint:

```json
{
  "name": "my-org/my-project",
  "version": "0.1.0",
  "deps": {
    "example/json-utils": "0.2.0"
  }
}
```

Then install:

```shell
moon install
```

## Version Constraints

Specify the version as a string in `moon.mod.json`. Use the exact version published on mooncakes.io.

## Using the Dependency

After installation, reference the package in your `moon.pkg.json` file's `"import"` section and use it in your `.mbt` source files.

In `moon.pkg.json`:

```json
{
  "import": [
    "example/json-utils"
  ]
}
```

## Key Constraints

- Only packages published on [mooncakes.io](https://mooncakes.io) can be added as dependencies
- Ensure the package is compatible with the `wasm` / `wasm-gc` backend — some MoonBit packages may only support native or JS targets
- After adding a dependency, always run `moon install` before `golem build`
- Check the package's documentation on mooncakes.io for usage examples and API reference
