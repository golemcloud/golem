---
name: golem-add-npm-package
description: "Adds npm package dependencies to Effect-based Golem projects. Use when the user asks to add a library, package, or dependency to an @golemcloud/effect-golem application."
---

# Adding an NPM Package Dependency to an Effect Golem Application

Effect Golem applications use normal npm dependencies, but application code is bundled into a
single ESM module and runs in a QuickJS-based WebAssembly component rather than Node.js. A runtime
dependency must therefore be compatible with both the Rollup build and the QuickJS/Wasm runtime.

## Steps

1. From the application root containing `package.json`, install the package:

   ```shell
   npm install <package-name>
   ```

   Root-level dependencies are shared by every Effect component in a multi-component application.
   Keep the resulting `package.json` and `package-lock.json` changes.

   For a build tool or type package that is not imported by component code, use a development
   dependency:

   ```shell
   npm install --save-dev <package-name>
   ```

2. Import the package from component source with its documented ESM or runtime-compatible entry
   point. Use a bare package specifier for npm packages; the emitted `.js` suffix is required for
   local source imports, not package imports.

3. Preserve the generated versions of `effect` and `@golemcloud/effect-golem`. They are
   externalized from the application bundle and supplied by the SDK's base Wasm, so upgrading one
   independently can create an incompatible or duplicate Effect runtime.

4. Build through Golem:

   ```shell
   golem build
   ```

   Do not run Rollup or TypeScript directly and do not edit generated files under `golem-temp/`.
   After a successful build, exercise the code in a deployed component because Node-based tests do
   not detect QuickJS-only runtime failures.

## Using a Package from an Effect Method

Agent handlers must still return an `Effect`. Wrap a synchronous package call in `Effect.sync` so
the package's result is the method's success value:

```typescript
import { Effect } from "effect";
import { camelCase } from "change-case";

format: ({ value }) => Effect.sync(() => camelCase(value)),
```

When adding the call to an existing Effect pipeline, return it from `Effect.map` (or replace the
pipeline's success value with `Effect.as`). For example, this preserves the state update and returns
the package result:

```typescript
increment: () =>
  Ref.updateAndGet(state, ({ count }) => ({ count: count + 1 })).pipe(
    Effect.map(() => camelCase("hello world")),
  ),
```

Keep the method contract aligned with the value returned by the handler; the example requires
`success: Schema.String`. Calling the package only inside `Effect.tap`, or using a block-bodied
`Effect.map` callback without `return`, does not make its value the method result.

## Runtime Compatibility Checklist

- Prefer packages implemented in JavaScript that publish an ESM entry point compatible with the
  generated Rollup pipeline.
- Reject packages that require native `.node` binaries, N-API, native C/C++ addons, or
  platform-specific processes. Those binaries cannot load inside the Wasm component.
- Check every transitive runtime dependency for Node built-ins or Web APIs that the QuickJS runtime
  does not provide. The presence of `@types/node` only supplies types; it does not make every Node
  API available at runtime.
- Do not assume a package works merely because Rollup can bundle it. Build success verifies module
  and type compatibility, while deployment and invocation verify runtime compatibility.
- Consult the [wasm-rquickjs documentation](https://github.com/golemcloud/wasm-rquickjs) for the
  current runtime API and package compatibility information.

Build-only development dependencies run in the local Node toolchain and do not need QuickJS
compatibility unless application source imports them into the component bundle.

## Bundling Behavior

The generated build uses Rollup to resolve npm modules, convert CommonJS dependencies, support JSON
imports, inline dynamic imports, and emit one ESM module. Ordinary third-party dependencies are
bundled into that module. `effect`, `@golemcloud/effect-golem`, the SDK database subpaths, and Golem
or WASI host modules remain external because the base Wasm provides them.

If the build fails, check the package's exports and prefer its ESM or runtime-compatible entry
point. If the component fails only when invoked, look for a native addon, an unresolved Node
built-in, an unsupported Web API, or dynamic module loading that Rollup could not inline. Replace
the package with a pure-JavaScript, Wasm-compatible alternative when necessary.

## Already Available Packages

Generated Effect applications already include these dependencies; do not add them again:

- `@golemcloud/effect-golem` and `effect`
- `rollup` and the generated Rollup plugins
- `typescript`, `tslib`, and `@types/node` as development dependencies

For database access, do not install native Node drivers such as `pg`, `mysql2`, or
`better-sqlite3`. Use the host-backed adapters from `@golemcloud/effect-golem/sqlite`,
`@golemcloud/effect-golem/postgres`, `@golemcloud/effect-golem/mysql`, or
`@golemcloud/effect-golem/ignite2` as appropriate.

AI and LLM npm clients are subject to the same rules: verify that the selected package and all of
its transitive dependencies can be bundled and use only APIs available in the QuickJS/Wasm
runtime.
