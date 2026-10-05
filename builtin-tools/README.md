# Built-in tools

Built-in tools and middlewares are component sources built from this repository, published as
immutable releases in [`golemcloud/golem-builtins`](https://github.com/golemcloud/golem-builtins),
downloaded into the registry-service artifact cache, and provisioned at registry startup.
`BUILTIN_EXPORTS` in `golem-registry-service/src/services/builtin_tool_provisioner.rs` is the
production export inventory; `builtin-artifacts.lock.json` pins each component artifact's
independent version and SHA-256. The registry derives the canonical GitHub release URL from that
lock. Server configuration is only for deployment-specific cache placement and source overrides;
it does not duplicate the production release catalog.

The filesystem component is implemented in Rust under `builtin-tools/filesystem-tools/`. The same
component provides the `read-file`, `write-file`, `edit-file`, `ls`, and `grep` tools and the
universal `path-policy` middleware.

The reusable Audit policy is implemented under `builtin-tools/audit-middleware/`. Its package
README defines the sink and idempotency contract and includes a runnable duplicate-occurrence
binding.

The reusable output-redaction middleware lives under `builtin-tools/output-redaction/`. It is
published and installed through the normal tool middleware release and binding flow; unlike the
provisioned filesystem releases, it is not automatically available to environments. Its README
defines the supported selector and literal-pattern policy and includes a complete binding example.

The reusable persistent rate-limit middleware lives under `builtin-tools/rate-limit-middleware/`.
It uses one durable backend agent per policy to serialize fixed-window admission across owners and
records decisions by logical invocation ID so replay does not charge an invocation twice.

The JavaScript and TypeScript tools are implemented under `builtin-tools/js-ts-tools/`. The
`javascript-tools` component provides `node`, `npm`, and `npx`; the `typescript-tools` component
provides `tsc`. `node` provides JavaScript execution through Golem's QuickJS-based Node-compatible
runtime. The upstream npm and tsc executable graphs are Rollup-bundled and embedded in their
components; they do not provision an npm or TypeScript package tree into the invoking agent.
TypeScript's standard-library declarations and npm's manual pages are embedded as private read-only
data. Both components must use the `optimized` TypeScript preset so Wizer pre-initializes their
provider state.

## Adding a component-implemented built-in export
The web fetch tool is implemented separately under `builtin-tools/web-fetch/`. Its generated
`builtin-tools/web-fetch.wasm` is published externally rather than committed. It provides the
read-only, open-world `web-fetch` tool for bounded HTTP and HTTPS retrieval without filesystem
access. Its timeout, response-size, and redirect limits are optional invocation arguments, not
deployment configuration. HTML is returned as decoded source unless the invocation enables
conversion to readable text.

The Bash tool is implemented under `builtin-tools/bash/` and built reproducibly, in a pinned
container, into `builtin-tools/bash.wasm` (`cargo make build-bash-tool`). The `bash` component
provides the `bash` tool; see [its contract and examples](bash/README.md).

The Git tool is implemented in TypeScript under `builtin-tools/git/`, backed by pinned
`isomorphic-git`, and built into `builtin-tools/git-tool.wasm`. Release `git@0.1.7` supports local
`init`, `status`, `diff`, `log`, `branch`, `add`, `commit`, `checkout`, and narrow local `config`
workflows. It exposes no remote commands. The runtime provides WASI HTTP, and a separate integration
probe verifies that explicitly injecting `isomorphic-git/http/web` uses durable WASI HTTP calls
without executing socket calls. Any future network command must use that adapter path.

The initial Git release intentionally does not support remotes, merge/rebase, stash, reset/clean,
hooks, signing, submodules, linked worktrees, force operations, revision-expression syntax, or
pathspec magic. Checkout of paths after `--` is destructive and restores from the index. Branch
checkout rejects local changes it would overwrite. Diff output and computation have explicit
limits and fail instead of returning a truncated patch.

Executable-file modes cannot currently be persisted across separate TypeScript runtime instances.
Git operations fail closed when a `100755` entry is observable rather than silently changing it.
Relative symlinks are supported. File-to-directory checkout transitions are also rejected because
the pinned library cannot apply them safely.

## Adding a component-implemented built-in tool

1. Add a standalone tool component source and build it through its Golem application manifest. Do
   not invoke its language compiler directly.
2. Add its logical artifact ID to `BuiltinExportDescriptor`; keep export metadata in the descriptor
   and add the component name, independent artifact version, and checksum to
   `builtin-artifacts.lock.json`.
3. Set `component_name`, `export_name`, `release_version`, and `kind` to the artifact's exported
   metadata. Provisioning validates these values before writing anything.
4. Add an independent build task and include it in `build-builtin-tools`. Validate the generated
   WASM and run the registry provisioning tests.
5. Commit the exact source and build inputs, then manually publish an immutable component release
   from that clean local checkout with `cargo make publish-builtin-artifact`. Commit the resulting
   version/checksum update to `builtin-artifacts.lock.json` separately. Generated WASMs are never
   committed.

Provisioning is idempotent for identical bytes and an identical exact version. A published system
release is protected and immutable. Every byte-changing component publication requires a new
artifact version; a changed export definition or implementation also requires a new tool or
middleware version. Unchanged published export coordinates remain attached to their original
component revision. New component artifacts start at `0.0.1`. Until an artifact defines a separate
compatibility policy, increment its patch version for every byte-changing publication. Artifact
versions do not determine the versions of the tools or middlewares exported by the component.

Component-implemented built-ins are grantable registry releases, not ambient tools. A consuming
manifest must select each exact tool release under `tools.<name>.release` and each middleware
release under `tools.middleware.<name>.release`. Native tools compiled into the host use separate
registry/executor startup inventories and are ambient, so they have no top-level release
declaration. See `golem-native-tool/README.md` for that path.

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
      version: 0.4.0
  middleware:
    path-policy:
      release:
        account: builtin-tool-owner@golem.cloud
        name: path-policy
        version: 0.1.0

environments:
  local:
    tools:
      middleware:
        - name: path-policy
          version: 0.1.0
          parameters:
            base: /workspace
            allowed_roots:
              - path: project
                operations: [read]
          filesystemAccess: allowed

agents:
  FileReader:
    tools:
      read-file:
        filesystemAccess: allowed
```

This assumes the component exports the `FileReader` agent and that its files are rooted under
`/workspace`. The policy resolves relative invocation paths and relative allowed roots against
`base`. Each root independently grants `read`, `write`, and/or `delete`; `read-file` and `ls` use
`read`, `grep` uses `read`, `write-file` and `edit-file` use `write`, and `delete-file` uses
`delete`. Unknown tool names pass through unchanged. Recognized filesystem tool names are accepted
only at their root command and must have a string `path` argument.

Both filesystem grants are intentional. The agent's `read-file` binding needs
`filesystemAccess: allowed` so the leaf tool can read the owner filesystem. The `path-policy`
middleware occurrence independently needs `filesystemAccess: allowed` so it can inspect existing
path components and reject symbolic links. Without the middleware grant, protected calls fail
closed with `path-policy-denied` before leaf dispatch because the policy cannot inspect the owner
filesystem root.

`ls` and `grep` intentionally discover paths below their explicit root argument. A middleware that
allows such a call authorizes inspection of the selected subtree; include/exclude globs select
results but are not an authorization boundary. The tools never follow symbolic links.

The middleware normalizes `.` and `..` components without allowing traversal above the owner root,
checks containment by path components, rejects backslashes and NUL bytes, and rejects any existing
symbolic-link component, including a dangling final link. A write below a nonexistent suffix is
allowed only when its nearest existing ancestor is link-free and the normalized target remains
inside a write-enabled root. Allowed calls receive the normalized absolute path; denied calls
return `path-policy-denied` before invoking the wrapped tool.

This is argument-level policy for the named filesystem tools, not a filesystem sandbox. It does
not restrict arbitrary filesystem behavior inside a wrapped tool and does not prevent another
component with its own filesystem capability from accessing paths directly.
This assumes the component exports the `FileReader` agent. The built-in filesystem tools require
filesystem access but provision no files of their own. This example explicitly grants
`filesystemAccess: allowed` on the agent binding; if it is omitted and no grant is inherited,
deployment fails with an error requesting that permission. This fails closed rather than exposing
the agent's filesystem.

The Git tool uses the same exact-release selection and explicit filesystem grant:

```yaml
tools:
  git:
    release:
      account: builtin-tool-owner@golem.cloud
      name: git
      version: 0.1.7

agents:
  CodingAgent:
    tools:
      git:
        filesystemAccess: allowed
```

Examples of the supported command surface include `git -C workspace status --short`,
`git -C workspace diff --cached -- src/main.ts`, `git -C workspace add -- src/main.ts`,
`git -C workspace commit -m "Fix validation"`, and
`git -C workspace checkout -b fix-validation`. Repeated `-C` values are applied in order. Because
the shared tool command model requires canonical long names, short-only Git options also have
descriptive long forms such as `--working-directory` and `--new-branch`; inherited `-C` is accepted
after a subcommand as well as before it.

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
