# yq built-in tool

`yq@0.1.0` runs [Mike Farah's yq](https://github.com/mikefarah/yq) v4.54.3 as a Golem tool, built
from Go with the Go SDK and `componentize-go`. It queries and edits YAML, JSON, XML, TOML, CSV,
TSV, properties, HCL and Lua with the upstream command line.

## Command

The input has `cwd` (absolute, default `/`, bound to Bash's `PWD`) and yq's own arguments after
`--`. The arguments are yq's command line without the program name; as in upstream yq, an unknown
first word means `eval`:

```shell
yq --cwd /workspace -- '.spec.replicas' deploy.yaml
yq --cwd /workspace -- -i '.version = "2"' chart.yaml
yq -- eval-all '. as $item ireduce ({}; . * $item)' base.yaml override.yaml
```

Optional stdin is read for `-` or when no file is given. stdout and stderr are yq's. The result is
the shared `ProcessExitStatus`: 0 on success and 1 on any yq error, as natively.

File arguments resolve against the owner's filesystem and need the tool's filesystem access.
In-place edits (`-i`) refuse a read-only file, as natively.

The `system` operator stays behind yq's own `--security-enable-system-operator` flag. Its command
runs as a call of the declared Golem tool with that name, with the command's arguments, the same
working directory and the operator's input on stdin; it never starts a process. An undeclared
command fails as a missing executable does natively.

The IANA time-zone database is embedded for `tz()`.

## Source

The module resolves `github.com/mikefarah/yq/v4` to the fork
[`golemcloud/yq`](https://github.com/golemcloud/yq) at tag `v4.54.3-golem.2`, which is upstream
`v4.54.3` plus three commits on branch `golem-yq-tool`:

- `Add stdio hooks and a wasip1 read-only check for Golem`: `yqlib.Stdin`, `Stderr`,
  `StdinIsPiped` and `StdoutIsTerminal` replace the process streams, and on `wasip1` the in-place
  read-only check asks the host with a write-only open, because WASI reports no permission bits.
- `Add a hook for the system operator's command runner`: `yqlib.RunSystemCommand` replaces
  `os/exec` for the `system` operator.
- `Build the completion command per root command`: each `cmd.New()` gets its own `completion`
  command, so completion scripts go to that command's output writer.

yq is MIT licensed. Its module dependencies are MIT, Apache-2.0, BSD-2-Clause, BSD-3-Clause and,
for `hashicorp/hcl/v2` only, MPL-2.0.

## Differences from native yq

Some file error messages quote a different errno text from the WASI runtime.
