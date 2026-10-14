---
name: golem-generating-tool-clients
description: "Generates typed clients for existing built-in tools and MCP imported tools. Use when selecting tool releases or imports, generating clients with golem build, or wiring generated clients into Rust, TypeScript, Effect, Scala, MoonBit, or Go consumers."
---

# Generate typed clients for existing tools

Use a generated client to call an existing tool without copying its schema into the
consumer. Add the tool to the consumer's `dependencies.tools` in `golem.yaml`.
`golem build` automatically generates its client in the consumer's language, like
an agent client selected through `dependencies.agents`. Import that client into
the consumer and configure its tool binding separately.

## Dependencies and tool bindings

Prefer `components.<consumer>.dependencies.tools` for a typed tool client. Keep
the consumer's language template: the build uses it to select the generator.
For a custom template, set `componentTemplates.<template-name>.guestLanguage`
and apply that template to the consumer. No additional
`bridge.<language>.internal.tools` entry is needed.

These manifest entries have different roles:

| Entry | Role |
|-------|------|
| Top-level `tools.<name>` | Selects a registry release or local implementing component; MCP imported and ambient native names need no declaration |
| `components.<consumer>.dependencies.tools` | Requests automatic client generation during `golem build`; local providers build before their consumers |
| `components.<consumer>.tools` or an agent's `tools` | Binds the tool for runtime invocation; a binding alone does not request generation |

A dependency does not add a runtime binding. A binding does not generate a client
or wire a generated package into the consumer's compiler configuration. Neither
replaces the caller's runtime permissions or the deployer's authority. The build
does not scan source code to infer dependencies from tool calls.

For a tool defined in another component, declare the provider and depend on the
tool's name:

```yaml
tools:
  echo:
    component: my-app:provider

components:
  my-app:consumer:
    dependencies:
      tools: [echo]
    tools:
      echo: {}
```

Keep both components' existing directories and language templates. A structured
dependency `{ component: 'my-app:provider', name: echo }` also identifies a local
tool. Do not add a guest-client dependency on the consumer itself: self-dependencies
are rejected. For a definition owned by the same component, use the language SDK's
definition-owned client described in its call-tool guide.

Run commands from the application root. The examples assume the selected
environment is `local`. To select it explicitly, use
`golem --environment local build --yes` or the same global option before other
commands.

## Generate a built-in tool client

This example uses a TypeScript component in `consumer/` and the built-in `bash`
release. Keep the component's existing templates and build settings. Merge these
entries into the application manifest:

```yaml
tools:
  bash:
    release:
      account: builtin-tool-owner@golem.cloud
      name: bash
      version: "0.2.1"

components:
  my-app:consumer:
    dir: consumer
    templates: ts
    dependencies:
      tools: [bash]
    tools:
      bash:
        filesystemAccess: allowed
```

The dependency automatically generates the TypeScript client. The component
binding exposes `bash` to its agents and permits filesystem access. Use an agent
binding instead when only one agent needs the tool. Bindings do not replace the
calling agent's runtime permissions or the deployer's authority.

The release declaration selects the registry metadata used for generation. A build
can create the environment grant needed to read that metadata; the CLI shows the
grant plan before applying it. Authenticate to the selected server with an account
that can make those grant changes. The release must exist on that server.

Deployment default tools are not build selections. Although a normal deployment
adds `bash` by default, declare `tools.bash` to generate its registry client. This
explicit declaration also replaces the deployment default, so supply its binding
yourself. If the server instead provides an ambient native tool, select its
canonical name without a registry declaration; generation reads its metadata from
the selected environment.

Generate before adding the new import to consumer code:

```shell
golem build --yes
```

With the default temporary directory, the TypeScript source is
`golem-temp/bridge-sdk/ts/internal/bash-tool-guest-client/bash-tool-guest-client.ts`.
The generator also writes `package.json` and `tsconfig.json`. It does not publish
this package to npm.

For a consumer in `consumer/`, merge this source alias and include pattern into
`consumer/tsconfig.json`. Preserve its other paths and include entries:

```json
{
  "compilerOptions": {
    "paths": {
      "bash-tool-guest-client": [
        "../golem-temp/bridge-sdk/ts/internal/bash-tool-guest-client/bash-tool-guest-client.ts"
      ]
    }
  },
  "include": [
    "src/**/*.ts",
    "../golem-temp/bridge-sdk/ts/internal/bash-tool-guest-client/*.ts"
  ]
}
```

Call the generated client inside an agent method:

```typescript
import { BashClient } from 'bash-tool-guest-client';

const result = await BashClient.newClient().run("printf 'hello\\n'", '', 600);
if (result.exitCode !== 0) {
  throw new Error(`bash exited with ${result.exitCode}: ${result.stderr}`);
}
return result.stdout;
```

The generated arguments are positional: `script`, `cwd`, and `timeout` in seconds.
Pass all three. The empty `cwd` selects the default working directory. Bash returns
stdout and stderr as strings in the result, not output stream handles. A nonzero
shell exit code is a result, distinct from a declared tool or RPC failure.

Run `golem build --yes` again to compile the consumer, then
`golem deploy --yes` to deploy the release selection, binding, and consumer.

## Generate an MCP imported tool client

Start with an MCP server that advertises a tool through `tools/list`. This example
assumes an upstream command named `catalog.lookup` with an `inputSchema` containing
required string `item-id` and optional boolean `includeHistory`, no additional
properties, and an `outputSchema` containing string `id` and integer `revision`.
Do not reproduce those schemas in consumer code: discovery supplies them.

Merge the following into a TypeScript application's manifest. Replace the example
URL with the server's Streamable HTTP endpoint:

```yaml
mcp:
  imports:
    local:
      - url: https://tools.example.com/mcp
        include: [catalog-lookup]

components:
  my-app:consumer:
    dir: consumer
    templates: ts
    dependencies:
      tools: [catalog-lookup]
    tools:
      catalog-lookup: {}
```

Upstream names are sanitized to kebab-case before filtering: `catalog.lookup`
becomes `catalog-lookup`. An import `prefix: remote` would make the canonical name
`remote-catalog-lookup`; change the dependency and binding to
that name too. Application and ambient native tools take precedence over imported
names, and earlier imports take precedence over later imports. No top-level
`tools.catalog-lookup` declaration is needed.

Select the environment whose import list you configured. The CLI resolves the
current manifest's imports through that environment before generating clients,
not through an older deployment. The server needs access to the MCP endpoint and
usable metadata. The endpoint must accept the supported MCP import protocol
described in the MCP imports reference; reachability alone is not enough.
The calling agent also needs network permission for that endpoint
and, for OAuth refresh, the token endpoint.

For an OAuth-protected import, configure its `securityScheme`, create the selected
environment and security scheme, and complete consent before generation:

```shell
golem api mcp-import authorize 0 --manifest
# Open the printed URL, then copy the full callback URL.
golem api mcp-import complete 0 --manifest '<callback-url>'
golem api mcp-import status 0 --manifest
```

The index is zero-based in the selected environment's import list. Keep the import
and its rendered environment variables unchanged during consent. For bearer or
basic authentication, use environment-variable templates for secret values, not
literal credentials in committed manifests. For example, set `auth.bearer` to
`"{{ MCP_TOKEN }}"` and supply `MCP_TOKEN` in the CLI environment. Generated clients
contain neither credentials nor the MCP URL.

Run `golem build --yes` before adding the new consumer import. The generated source
is `golem-temp/bridge-sdk/ts/internal/catalog-lookup-tool-guest-client/catalog-lookup-tool-guest-client.ts`.
Wire it into `consumer/tsconfig.json` as above, replacing the package and file name.

For this upstream schema, call the generated client from an agent method:

```typescript
import { CatalogLookupClient } from 'catalog-lookup-tool-guest-client';

const invocation = CatalogLookupClient.newClient().catalog_lookup(false, 'item-123');
const collected = await invocation.collect();
if (collected.result.status === 'rejected') throw collected.result.reason;
if (collected.stdout.status === 'rejected') throw collected.stdout.reason;
if (collected.stderr.status === 'rejected') throw collected.stderr.reason;

const result = collected.result.value;
const text = collected.stdout.value === undefined
  ? ''
  : new TextDecoder().decode(collected.stdout.value);
return { id: result.structured.id, text };
```

The generated method name and argument order come from the projected definition;
inspect the generated signature for your server. Here the method starts a streaming
invocation synchronously. `collect()` drains stdout and stderr concurrently and
preserves each outcome separately from the result. Check all three settlements.
Stderr bytes alone do not mean failure.

MCP results have `structured` and `content`. With `outputSchema`, `structured` has
the generated schema type; otherwise it is optional JSON text. A single text,
image, or audio block uses stdout and a content descriptor. Multiple blocks and
resources remain ordered in `content`. Do not assume every MCP call returns text,
or discard `content` in an application that needs those blocks. Rebuild the
consumer and deploy the import and binding with `golem deploy --yes`.

## Other supported languages

Tool clients are internal guest bridges. The consumer's `rust`, `ts`, `effect`,
`scala`, `moonbit`, or `go` language template selects its dependency client generator.
With default paths, dependencies generate under
`golem-temp/bridge-sdk/<language>/internal/<tool-name>-tool-guest-client/`.

| Language | Generated files and consumer entry point for `bash` | Consumer wiring |
|----------|---------------------------------------------------|-----------------|
| TypeScript | `bash-tool-guest-client.ts`; `BashClient.newClient()` | Source alias and include pattern as above |
| Effect | `bash-tool-guest-client.ts`; exported `client` | Source alias and include pattern; use the Effect SDK |
| Rust | `Cargo.toml`, `src/lib.rs`; `bash_tool_guest_client::BashClient::new()` | Local Cargo path dependency |
| Scala | sbt project; `golem.bridge.client.bash.BashClient()` | Include the generated Scala sources in the consumer build |
| MoonBit | `moon.mod.json`, `client/moon.pkg`, `client/client.mbt`; `@bash.BashClient::new()` | Local module dependency and `bash-tool-guest-client/client` package import |
| Go | Go module `golem.local/bridge/bash-tool-guest-client`; `bash.Run.Call(func(a *bash.RunArgs) { a.Script = ... })` | `require` and `replace` of the generated module in the consumer's `go.mod` |

Use the language's call-tool guide for invocation and error handling:
`golem-call-tool-ts`, `golem-tools-middleware-effect`, `golem-call-tool-rust`,
`golem-call-tool-scala`, `golem-call-tool-moonbit`, and `golem-call-tool-go`.

## Explicit bridge generation

Use `bridge.<language>.internal.tools` only when you need a generation target
separate from the consumer's dependency, such as another language or output
directory. Do not repeat the dependency selection in ordinary consumer examples.

```yaml
bridge:
  rust:
    internal:
      tools: [bash]
```

An explicit bridge `outputDir` changes this target to
`<outputDir>/<tool-name>-tool-guest-client/`; component dependency targets retain
the default dependency path. An explicit selection does not add a runtime binding.
Use `golem build`, not a tool selector on `golem generate-bridge`: that command
does not expose one.

## Regenerate and troubleshoot

After changing a selected release or an upstream tool definition, run
`golem build --yes` again and adapt the consumer to the generated signatures.
The generation cache includes the selected release and metadata identity. Never
edit the generated client or copy a stale schema to bypass discovery. Use
`golem build --yes --force-build` when diagnosing stale build output.

| Failure | What to check |
|---------|---------------|
| Default `bash` has no generated client | Declare its release and add it to the consumer's tool dependencies; deployment defaults are not build selections |
| Release metadata cannot be read | Check release coordinates, selected server/environment, and authority to reconcile its grant |
| An imported name cannot be resolved | Check the upstream list, sanitized name, prefix, filter, precedence, and selected environment |
| MCP metadata or consent is missing | Make the endpoint reachable and complete OAuth consent for the exact manifest target |
| Unsupported protocol version | Check that the upstream endpoint accepts the supported MCP import protocol; discovery fails before client generation |
| The compiler cannot resolve the client package | Check the generated directory and consumer source alias or local dependency |
| Generation works but invocation is denied | Deploy the tool binding and check the agent's tool/resource permissions separately |

## Related Skills

- `golem-edit-manifest` for manifest configuration
- `golem-build` and `golem-troubleshoot-build` for build options and diagnostics
- `golem-configure-mcp-server` for exporting Golem tools to MCP, rather than importing them

## Reference

- [Application manifest](https://learn.golem.cloud/next/app-manifest) for release declarations, bindings, default tools, and bridge selectors
- [MCP imports](https://learn.golem.cloud/next/invoke/mcp#importing-tools-from-another-mcp-server) for discovery, consent, authentication, and metadata refresh
- [Tool authoring](https://learn.golem.cloud/next/develop/tool-authoring#drain-output-attachments-concurrently) for independent output channels
- [Tool security](https://learn.golem.cloud/next/develop/tool-security) for tool and resource permissions
