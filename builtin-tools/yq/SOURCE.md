# Source and licenses

`yq.wasm` is built from this directory by `cargo make build-yq-tool`. golem-cli pins Golem's Go
toolchain `go1.27.1-golem.5` (`go_toolchain::TAG` in `cli/golem-cli/src/versions.rs`) and Golem's
preview1 adapter (`go_adapter`), which it passes to `componentize-go` with `--adapt`. `go.mod` pins
`componentize-go` v0.4.3 in its `tool` directive.

`licenses/Golem-LICENSE` is the repository license for the tool implementation in this directory.
The Go SDK modules `sdks/go/golem` and `sdks/go/core` are Apache-2.0; their license is in
`licenses/THIRD_PARTY_LICENSES.txt`.
`licenses/THIRD_PARTY_LICENSES.txt` holds the license, notice and patent files of every Go module
linked into the component, of the Go runtime and standard library, and of the
`wasi_snapshot_preview1` adapter. The adapter's Apache-2.0 WITH LLVM-exception text is taken from
`componentize-go`, which ships the same text as wasmtime. Regenerate it from this directory
when `go.mod`, `go.sum` or the toolchain changes, with Golem's Go toolchain on `PATH`:

```shell
{
  echo "Third-party licenses of yq.wasm"
  echo
  echo "Generated from \`go list -deps\` of this module for GOOS=wasip1 GOARCH=wasm; see ../SOURCE.md."
  GOOS=wasip1 GOARCH=wasm go list -deps -f '{{with .Module}}{{if ne .Path "golem-yq"}}{{.Path}} {{.Version}} {{with .Replace}}{{.Path}} {{.Version}}{{end}}|{{.Dir}}{{end}}{{end}}' . | sort -u |
  while IFS='|' read -r name dir; do
    for f in $(ls "$dir" | grep -i -E '^(licen[cs]e|copying|notice|patents)(\.(md|txt))?$'); do
      printf '\n================================================================\n%s\n%s\n----------------------------------------------------------------\n' "$name" "$f"
      cat "$dir/$f"
    done
  done
  for f in LICENSE PATENTS; do
    printf '\n================================================================\nGo runtime and standard library, golemcloud/go go1.27.1-golem.5\n%s\n----------------------------------------------------------------\n' "$f"
    cat "$(go env GOROOT)/$f"
  done
  printf '\n================================================================\nwasi_snapshot_preview1 adapter, golemcloud/wasmtime golem-adapter-v50.0.1-1\nLICENSE (Apache-2.0 WITH LLVM-exception, as shipped by componentize-go v0.4.3)\n----------------------------------------------------------------\n'
  cat "$(go list -m -f '{{.Dir}}' github.com/bytecodealliance/componentize-go)/LICENSE"
} > licenses/THIRD_PARTY_LICENSES.txt
```

## Modified and MPL-2.0 sources

- yq is the MIT-licensed fork [`golemcloud/yq`](https://github.com/golemcloud/yq) at tag
  `v4.54.3-golem.2`, which is upstream `v4.54.3` with the changes listed in `README.md`.
- `go.bytecodealliance.org/pkg` is the Apache-2.0 fork
  [`golemcloud/go-pkg`](https://github.com/golemcloud/go-pkg) at tag `v0.2.3-golem.2`, which is
  upstream v0.2.3 with the patches described in its `GOLEM.md`, such as resuming an async task
  when a Go timer is due.
- The Go runtime and standard library are the BSD-3-Clause fork
  [`golemcloud/go`](https://github.com/golemcloud/go) at tag `go1.27.1-golem.5`, which is Go 1.27.1
  with the runtime patches described in `cli/golem-cli/src/versions.rs`. The regeneration command
  names this tag; change it together with `go_toolchain::TAG`.
- The preview1 adapter is `wasi_snapshot_preview1.reactor.wasm` from the Apache-2.0 WITH
  LLVM-exception fork [`golemcloud/wasmtime`](https://github.com/golemcloud/wasmtime) at tag
  `golem-adapter-v50.0.1-1`. It requests `mutate-directory` on directories the guest opens, which
  upstream's adapter does not. The regeneration command names this tag; change it together with
  `go_adapter::TAG`.
- `github.com/hashicorp/hcl/v2` v2.25.0 is under the Mozilla Public License 2.0. The component
  contains it unmodified. Its source is available at
  <https://github.com/hashicorp/hcl/tree/v2.25.0> and from the Go module proxy as
  `github.com/hashicorp/hcl/v2@v2.25.0`.
- The other modules are under MIT, Apache-2.0, BSD-2-Clause or BSD-3-Clause, and their exact
  versions are pinned in `go.sum`.
