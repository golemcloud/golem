---
name: golem-add-moonbit-package
description: "Adds a MoonBit module and package import to a Golem project. Use when adding a mooncakes dependency or importing one of its packages."
---

# Add a MoonBit dependency

Current MoonBit projects declare module dependencies in `moon.mod` and package imports in
`moon.pkg`. Golem generates these formats for applications and bridge modules. Do not create
`moon.mod.json` or `moon.pkg.json`.

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

## Use a local module or generated bridge

Golem applications include `moon.work`. Keep the application member and any local SDK member,
then add the local module directory to `members`:

```moonbit
members = [
  ".",
  "golem-temp/bridge-sdk/moonbit/internal/weather-agent-guest-client",
]
```

Add `"weather-agent-guest-client@0.0.1"` to the application's `moon.mod` imports and
`"weather-agent-guest-client/client" @weather` to the caller's `moon.pkg` imports. The workspace
resolves the module locally. Do not edit the generated bridge's `moon.mod` or put local paths in
the application's module imports. Generate the bridge through `golem build --yes` before running
standalone MoonBit checks that need it.

The stock Golem build template supports this workspace layout. Moon places the core Wasm under
`_build/wasm/<profile>/build/<module-name>/<package-path>/`. The application name sets both the
generated module name and this artifact directory; component namespaces may differ. Adding members
does not require custom build or embed commands.

## Build and code generation

`golem build --yes` prepares dependency sources with `moon tree --package --target wasm` before
code generation. Published SDK tools run as `moonx golemcloud/golem_sdk_tools/cmd@<version> --`;
the selected version must have its prebuilt Wasm asset published to the registry. Do not add
`bin-deps` or invoke launchers under `_build/__moonbin__`. A local SDK override runs the sibling
tools checkout with `moon -C <tools-path> run cmd --` instead.

For an ordinary registry dependency, run `moon check --target wasm` and `golem build --yes`. Confirm that the
dependency supports the component's `wasm` target; native-only and JavaScript-only packages cannot
be linked into a Golem agent.
