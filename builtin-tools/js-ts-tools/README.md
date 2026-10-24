# JavaScript and TypeScript command tools

The `node`, `npm`, `npx`, and `tsc` tools return `ProcessExitStatus`, not a bare integer.
The TypeScript SDK exports the `ProcessExitStatus` type and `processExitStatusSchema` from
`@golemcloud/golem-ts-sdk`. Use the schema in a tool's `.returns(...)` declaration and return
`ok({ exit_code: code })`. The schema has the named root Ref `golem.tool.ProcessExitStatus`
and the definition `record { exit_code: u8 }`. Only this nominal contract identifies a process
status; a record with the same fields is not a substitute.

The runner normalizes finite integer statuses modulo 256. For example, `-1` becomes `255`
and `256` becomes `0`. Fractional and non-finite statuses produce status 1 and a stderr diagnostic. Streams, arguments,
working directories, and process state restoration retain their existing behavior.

## TypeScript in `node` and `npx`

`node` runs `.ts`, `.mts`, and `.cts` files without flags. The runtime removes the types and
transforms enums and namespaces, but it does not type-check or read `tsconfig.json`; use `tsc`
for type checks. `node -e` source is read as JavaScript first and, when it does not parse, as
TypeScript. `--input-type=commonjs-typescript` reads it as TypeScript directly.

Before the script, or with `-e` before the first program argument or `--`, `node` accepts `--input-type=commonjs` and
`--input-type=commonjs-typescript`, and accepts `--experimental-strip-types`,
`--experimental-transform-types`, `--no-experimental-strip-types`, `--no-warnings`, and
`--enable-source-maps` as options that change nothing. `--input-type=module` and
`--input-type=module-typescript` fail, because `-e` source runs as an async function body and
cannot contain static `import` declarations; run a `.mjs` or `.mts` file instead. Any other
option fails with `node: bad option`.

When `tsx` or `ts-node` is not installed in the working directory, an ancestor, or the npm
global prefix, `npx tsx <file>`, `npx ts-node <file>`, and their `-e` forms run through `node`
instead of downloading the package. `ts-node` writes one stderr line that says type checking
is skipped; `ts-node -T` and `--transpile-only` are accepted. Other `tsx` and `ts-node` options, watch mode, and the REPL fail with a message
that points at `node <file>` and at installing the real package. `npx --package`, `npx --call`,
and an installed command always run through npm.

Inline evaluation started through the runtime's `node:child_process` adapter, for example from
a package script, does not accept the `*-typescript` input types.

## Child processes and bound tools

Scripts can use `node:child_process` to call tools bound to the calling component.
The command name and arguments use the same command schema as the Bash tool.
Local Node scripts and package launchers take precedence over a tool with the
same name. No host operating-system process is started.

Set `GOLEM_NODE_TOOL_PROCESSES=0` or `false` to disable this dispatch while keeping
local Node scripts available. The switch is checked for each call. It is a
compatibility option, not a permission control; Golem tool permissions still apply.
The shared TypeScript SDK template installs the adapter in both the main runtime
and fresh JavaScript execution runtimes. Its build requires the generic WRQJS
process-backend and runtime-init hooks.

Local Node children started by npm currently finish when synchronous script
evaluation returns. Use synchronous tool calls in those lifecycle scripts;
asynchronous work can outlive output capture. This does not affect asynchronous
tool calls awaited by the built-in `node` runner itself.

## Releases and builds

The source tool coordinates are `node@0.1.10`, `npm@10.9.9+golem.9`,
`npx@10.9.9+golem.9`, and `tsc@5.9.2+golem.9`. These require new immutable
component releases. The artifact lock still pins the previous releases until
authorized publication. Source builds do not publish them.

Build both components through `golem build -P optimized --force-build --yes` in this directory.
The optimized preset pre-initializes the bundled CLIs with Wizer.
