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

The tool coordinates are `node@0.1.9`, `npm@10.9.9+golem.8`,
`npx@10.9.9+golem.8`, and `tsc@5.9.2+golem.8`. The catalog selects these releases,
and the artifact lock pins their immutable component releases. Source builds do not
publish them.

Build both components through `golem build -P optimized --force-build --yes` in this directory.
The optimized preset pre-initializes the bundled CLIs with Wizer.
