# Built-in tools

Built-in tools are component sources built from this repository, published as immutable releases in
[`golemcloud/golem-builtins`](https://github.com/golemcloud/golem-builtins), downloaded into the
registry-service artifact cache, and provisioned at registry startup. `BUILTIN_TOOLS` in
`golem-registry-service/src/services/builtin_tool_provisioner.rs` is the production inventory;
`builtin-artifacts.lock.json` pins each component artifact's independent version and SHA-256. The
registry derives the canonical GitHub release URL from that lock. Server configuration is only for
deployment-specific cache placement and source overrides; it does not duplicate the production
release catalog.

The filesystem tools are implemented in Rust under `builtin-tools/filesystem-tools/` and built
into `builtin-tools/filesystem-tools.wasm`. The component provides the `read-file`, `write-file`, and
`edit-file` tools.

The JavaScript and TypeScript tools are implemented under `builtin-tools/js-ts-tools/`. The
`javascript-tools` component provides `node`, `npm`, and `npx`; the `typescript-tools` component
provides `tsc`. `node` provides JavaScript execution through Golem's QuickJS-based Node-compatible
runtime.
The upstream npm and tsc executable graphs are Rollup-bundled and embedded in their components;
they do not provision an npm or TypeScript package tree into the invoking agent. TypeScript's
standard-library declarations and npm's manual pages are embedded as private read-only data. Both
components must use the `optimized` TypeScript preset so Wizer pre-initializes their provider state.

The web fetch tool is implemented separately under `builtin-tools/web-fetch/` and built into
`builtin-tools/web-fetch.wasm`. It provides the read-only, open-world `web-fetch` tool for bounded
HTTP and HTTPS retrieval without filesystem access. Its timeout, response-size, and redirect limits
are optional invocation arguments, not deployment configuration. HTML is returned as decoded
source unless the invocation enables conversion to readable text.

## Adding a component-implemented built-in tool

1. Add a standalone tool component source and build it through its Golem application manifest. Do
   not invoke its language compiler directly.
2. Add its logical artifact ID to `BuiltinToolDescriptor`; keep tool metadata in the descriptor and
   add the component name, independent artifact version, and checksum to
   `builtin-artifacts.lock.json`.
3. Set `component_name`, `tool_name`, and `release_version` to the artifact's exported metadata.
   Provisioning validates these values before writing anything.
4. Add an independent build task and include it in `build-builtin-tools`. Validate the generated
   WASM and run the registry provisioning tests.
5. Commit the exact source and build inputs, then manually publish an immutable component release
   from that clean local checkout with `cargo make publish-builtin-artifact`. Commit the resulting
   version/checksum update to `builtin-artifacts.lock.json` separately. Generated WASMs are never
   committed.

Provisioning is idempotent for identical bytes and an identical exact version. A published system
release is protected and immutable: repeat startup with the same version only when the artifact and
metadata are identical. For any changed artifact or metadata, publish a new version; do not replace
the existing coordinate. New component artifacts start at `0.0.1`. Until an artifact defines a
separate compatibility policy, increment its patch version for every byte-changing publication;
artifact versions do not determine the versions of the tools or plugins exported by the component.

Component-implemented built-ins are grantable registry releases, not ambient tools. A consuming
manifest must select the exact release under `tools.<name>.release` **and** bind that logical name
under `agents.<agent>.tools`. Native tools compiled into the host use separate registry/executor
startup inventories and are ambient, so they have no top-level release declaration. See
`golem-native-tool/README.md` for that path.

For example, this selects the built-in `read-file` release and makes it available to an agent:

```yaml
manifestVersion: 1.6.0

app: filesystem-reader

components:
  example:filesystem-reader:
    templates: rust
    dir: .

tools:
  read-file:
    release:
      account: builtin-tool-owner@golem.cloud
      name: read-file
      version: 0.3.0

agents:
  FileReader:
    tools:
      read-file:
        filesystemAccess: allowed
```

This assumes the component exports the `FileReader` agent. The built-in filesystem tools require
filesystem access but provision no files of their own. This example explicitly grants
`filesystemAccess: allowed` on the agent binding; if it is omitted and no grant is inherited,
deployment fails with an error requesting that permission. This fails closed rather than exposing
the agent's filesystem.

The `web-fetch` release is selected and bound in the same way, but requires no filesystem grant:

```yaml
tools:
  web-fetch:
    release:
      account: builtin-tool-owner@golem.cloud
      name: web-fetch
      version: 0.1.0

agents:
  ResearchAgent:
    tools:
      web-fetch: {}
```
