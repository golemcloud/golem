// Package yqtool exposes Mike Farah's yq v4 command line as a Golem tool.
//
// The tool runs the upstream Cobra command with the invocation's arguments,
// working directory and streams. It never starts a process: the upstream
// `system` operator, still behind its own opt-in flag, runs its command as a
// call of the declared Golem tool with that name.
package yqtool

import (
	"bytes"
	"fmt"
	"io"
	"os"
	// Golem gives components no zoneinfo; tz() needs the IANA database.
	_ "time/tzdata"

	"github.com/golemcloud/golem/sdks/go/golem"
	"github.com/golemcloud/golem/sdks/go/golem/reflection"
	"github.com/golemcloud/golem/sdks/go/golem/tool"
	yqcmd "github.com/mikefarah/yq/v4/cmd"
	"github.com/mikefarah/yq/v4/pkg/yqlib"
)

// Yq is the tool's identity type.
type Yq struct{}

var Tool = tool.DefineTool[Yq]("yq", tool.Spec{
	Version: "0.1.0",
	Summary: "Mike Farah's yq v4.54.3: query and edit YAML, JSON, XML, TOML, CSV and properties",
})

type Args struct {
	Cwd  string
	Args []string
	In   io.Reader
}

var Run = Tool.OutputBody[Args, golem.ProcessExitStatus](func(a *Args, s *tool.CommandSpec) {
	s.Doc("Run yq with the given arguments, as `yq [eval|eval-all] [flags] expression [files]`")
	s.Option(&a.Cwd).Default("/").Env("PWD").Doc("working directory for relative file arguments")
	s.Tail(&a.Args).Separator("--").Verbatim().Doc("yq's own arguments")
	s.Stdin(&a.In).Optional().Doc("input read for `-` or when no file is given")
	s.Stdout().Doc("yq's results")
	s.Stderr().Doc("yq's errors and warnings")
})

var _ = Run.Handle(func(ctx *tool.OutputContext, a Args) (golem.ProcessExitStatus, error) {
	return golem.ProcessExitStatus{ExitCode: run(ctx.Stdout(), ctx.Stderr(), a)}, nil
})

func run(stdout, stderr io.Writer, a Args) uint8 {
	if err := os.Chdir(a.Cwd); err != nil {
		fmt.Fprintf(stderr, "Error: cannot use working directory %s: %v\n", a.Cwd, err)
		return 1
	}

	var in io.Reader = bytes.NewReader(nil)
	if a.In != nil {
		in = a.In
	}
	piped := a.In != nil
	yqlib.Stdin = in
	yqlib.StdinIsPiped = func() bool { return piped }
	yqlib.StdoutIsTerminal = func() bool { return false }
	yqlib.Stderr = stderr
	yqlib.RunSystemCommand = func(command string, args []string, stdin io.Reader) ([]byte, string, error) {
		return runTool(a.Cwd, command, args, stdin)
	}

	c := yqcmd.New()
	c.SetIn(in)
	c.SetOut(stdout)
	c.SetErr(stderr)

	// Same default as upstream main: an unknown first word means `eval`.
	args := append([]string{}, a.Args...)
	if _, _, err := c.Find(args); err != nil && (len(args) == 0 || (args[0] != "__complete" && args[0] != "__completeNoDesc")) {
		args = append([]string{"eval"}, args...)
	}
	c.SetArgs(args)

	// Cobra prints a usage error to its output writer, which falls back to
	// stderr natively; SetOut sends it to stdout here. Print it ourselves,
	// after Cobra's error line, as the native command does.
	c.SilenceUsage = true
	failed, err := c.ExecuteC()
	if err != nil {
		if !failed.SilenceUsage {
			fmt.Fprint(stderr, fmt.Sprintln(failed.UsageString()))
		}
		return 1
	}
	return 0
}

// runTool runs a system operator command as a call of the declared tool with
// that name, the way the native operator runs a host command: the command's
// stdout is the result and a nonzero status fails with its stderr.
func runTool(cwd, command string, args []string, stdin io.Reader) ([]byte, string, error) {
	t, ok := reflection.DiscoverTool(command)
	if !ok {
		return nil, "", fmt.Errorf("exec: %q: executable file not found in $PATH", command)
	}
	var out, errs bytes.Buffer
	status := t.Bind().RunCommandLine(args, map[string]string{"PWD": cwd}, stdin, &out, &errs)
	if status != 0 {
		return out.Bytes(), errs.String(), fmt.Errorf("exit status %d", status)
	}
	return out.Bytes(), errs.String(), nil
}
