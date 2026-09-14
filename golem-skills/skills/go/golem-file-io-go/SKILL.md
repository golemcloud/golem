---
name: golem-file-io-go
description: "Reading and writing files from a Go Golem agent with the standard os/io packages. Use when the user asks to read files, write files, or do filesystem operations from Go agent code."
---

# File I/O in a Go Agent

## Overview

There is **no** Golem-specific filesystem API in the Go SDK — you use the Go standard library. Golem agents run on a WASI filesystem, and the standard `os` and `io` packages work against it out of the box (the same runtime that exposes `os.Getenv`/`os.Environ` also serves file operations). So read and write files with `os.ReadFile`, `os.WriteFile`, `os.Open`, `os.OpenFile`, etc., exactly as in ordinary Go.

To provision files *into* an agent's filesystem at startup, declare them in the `files:` section of `golem.yaml` — load `golem-add-initial-files` for that.

## Steps

1. **Import** `os` (and `io`/`bufio` as needed) in your agent's `impl` package.
2. **Read** provisioned files with `os.ReadFile` / write with `os.WriteFile`.
3. **Provision** any initial files via the `files:` block in `golem.yaml` (see `golem-add-initial-files`).

## Reading Files

```go
import "os"

// Text (or any) file → []byte
data, err := os.ReadFile("/data/config.json")
if err != nil {
	// handle: with golem.Must this would abort+retry the invocation
}
text := string(data)
```

## Writing Files

Only paths that are writable — files provisioned `read-write`, or paths not provisioned at all (e.g. `/tmp`) — can be written to.

```go
import "os"

err := os.WriteFile("/tmp/output.txt", []byte("Hello, world!"), 0o644)
```

### Appending

```go
import "os"

f, err := os.OpenFile("/tmp/agent.log", os.O_APPEND|os.O_CREATE|os.O_WRONLY, 0o644)
if err != nil { /* handle */ }
defer f.Close()
_, _ = f.WriteString("log line\n")
```

## Checking Existence and Listing

```go
import (
	"errors"
	"os"
)

if _, err := os.Stat("/data/config.json"); errors.Is(err, os.ErrNotExist) {
	// not present
}

entries, err := os.ReadDir("/data")
for _, e := range entries {
	_ = e.Name()
}
```

## Complete Agent Example

Definition (`agents/reader/reader.go`):

```go
// Package reader is the DEFINITION of a file-reading agent.
package reader

import "github.com/golemcloud/golem/sdks/go/golem"

type ID struct{ Name string }

type LogIn struct{ Message string }

var Agent = golem.DefineAgent[ID](golem.Spec{
	Name: "ReaderAgent", Description: "Reads a provisioned file and appends to a log",
})

var (
	Greeting = golem.DefineMethod[ID, golem.Unit, string]("greeting", golem.Desc("Read /data/greeting.txt"))
	Log      = golem.DefineMethod[ID, LogIn, golem.Unit]("log", golem.Desc("Append a line to /tmp/agent.log"))
)
```

Implementation (`agents/reader/impl/impl.go`):

```go
// Package impl is the IMPLEMENTATION of the file-reading agent.
package impl

import (
	"os"
	"strings"

	"myapp/agents/reader"

	"github.com/golemcloud/golem/sdks/go/golem"
)

type state struct{}

var agent = golem.Implement(reader.Agent, func(reader.ID) *state { return &state{} })

func init() {
	golem.Handle(agent, reader.Greeting, func(_ *golem.Context[state], _ golem.Unit) string {
		// golem.Must aborts the invocation if the read fails.
		data := golem.Must(os.ReadFile("/data/greeting.txt"))
		return strings.TrimSpace(string(data))
	})

	golem.Handle(agent, reader.Log, func(_ *golem.Context[state], in reader.LogIn) golem.Unit {
		f := golem.Must(os.OpenFile("/tmp/agent.log", os.O_APPEND|os.O_CREATE|os.O_WRONLY, 0o644))
		defer f.Close()
		golem.Must(f.WriteString(in.Message + "\n"))
		return golem.Unit{}
	})
}
```

`golem.Must` unwraps `(value, error)` and aborts the invocation (which then retries) on error — the fail-loud way to handle I/O errors when the caller should not observe them as a value.

## Provisioning Initial Files (`golem.yaml`)

Files are declared in the `files:` block and uploaded during deploy. Each entry has a `sourcePath`, an absolute `targetPath`, and optional `permissions`:

```yaml
agents:
  ReaderAgent:
    files:
      - sourcePath: ./data/greeting.txt
        targetPath: /data/greeting.txt
        permissions: read-only      # default; cannot be written to
      - sourcePath: ./data/scratch/
        targetPath: /var/scratch/
        permissions: read-write
```

See `golem-add-initial-files` for the full cascade (component / agent / preset levels), directory and remote sources, and merge modes.

## Key Constraints

- Use the **standard library** (`os`, `io`, `bufio`) — there is no SDK file API, and none is needed. The WASI filesystem backs it.
- Files provisioned with `read-only` (the default) **cannot be written**; write to `read-write` paths or unprovisioned paths like `/tmp`.
- `targetPath` values are **absolute** and unique across file entries.
- The filesystem is **per-agent-instance** and isolated; each instance has its own view.
- Initial files are declared at the **agent type** level in `golem.yaml`, not passed per instance at creation time.

### Related Skills

| Skill | When to Load |
|-------|--------------|
| `golem-add-initial-files` | Provision files into the agent filesystem via `golem.yaml` |
| `golem-add-agent-go` | Define the agent that does the file I/O |
| `golem-add-config-go` | Prefer typed config over files for structured settings |
