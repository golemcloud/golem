---
name: golem-add-scala-dependency
description: "Add a new library dependency to a Scala Golem project. Use when the user asks to add a library, package, or dependency."
---

# Add a Scala Library Dependency

## Important constraints

- Golem Scala components compile to WebAssembly via **Scala.js**. Only Scala.js-compatible libraries will work.
- The artifact must be a Scala.js artifact — use the `%%%` operator (triple percent) in `build.sbt` so sbt resolves the `_sjs1_` cross-published variant.
- Libraries that depend on JVM-specific APIs (reflection, `java.io.File`, `java.net.Socket`, threads, etc.) **will not work**.
- Pure Scala libraries and libraries published for Scala.js generally work.
- If unsure whether a library supports Scala.js, add it and run `golem build` to find out.

## Steps

1. **Add the dependency to `build.sbt`**

   In the component's `build.sbt`, add the library under `libraryDependencies`:

   ```scala
   libraryDependencies += "com.example" %%% "library-name" % "1.0.0"
   ```

   Use `%%%` (not `%%`) to get the Scala.js variant of the library.

2. **Build to verify**

   ```shell
   golem build
   ```

   Do NOT run `sbt compile` directly — always use `golem build`.

3. **If the build fails**

   - Check if the library publishes a Scala.js artifact. Look for `_sjs1_` in the Maven/Sonatype listing.
   - Check for JVM-only dependencies in the transitive dependency tree.
   - Look for an alternative library that supports Scala.js.

## Already available libraries

These are already in the project's `build.sbt` — do NOT add them again:

- `golem-scala-sdk` — Golem agent framework, durability, transactions
- `scala-js-dom` — DOM API bindings (if present)

## HTTP and networking

Use the Golem SDK's HTTP client utilities. Standard JVM networking (`java.net`) is **not available** in Scala.js/WASM.
