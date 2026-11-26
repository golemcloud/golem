// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

package tool_test

import (
	"encoding/json"
	"os"
	"reflect"
	"strings"
	"testing"

	"github.com/golemcloud/golem/sdks/go/golem"
	"github.com/golemcloud/golem/sdks/go/golem/reflection"
	"github.com/golemcloud/golem/sdks/go/golem/tool"
)

// The cross-SDK command-line corpus. golem-schema's tool::argv runs the same
// cases, so the Go parser cannot drift from the one every other caller uses.
const argvConformanceCorpus = "../../../../test-data/tool-argv-conformance/v1.json"

type ArgvProfile uint8

const (
	ArgvProfileDebug ArgvProfile = iota
	ArgvProfileRelease
)

type ArgvMode uint8

const (
	ArgvModeLive ArgvMode = iota
	ArgvModeDry
)

type ArgvDeployArgs struct {
	Target   string
	Artifact golem.Option[string]
	Profile  ArgvProfile
	Retries  golem.Option[uint32]
	Offset   golem.Option[int64]
	Color    string
	Output   golem.Option[string]
	Force    bool
	Cache    bool
	Verbose  uint32
}

type ArgvVcsGlobals struct {
	Cwd   string
	Quiet bool
}

type ArgvCommitArgs struct {
	ArgvVcsGlobals
	Message string
	Amend   bool
}

type ArgvStatusArgs struct {
	ArgvVcsGlobals
	Short bool
}

type ArgvExecArgs struct {
	Command []string
	Cwd     golem.Option[string]
	DryRun  bool
}

type ArgvRunArgs struct {
	Program string
	Args    []string
}

type ArgvCollectArgs struct {
	Include []string
	Tag     []string
	Env     map[string]string
	Label   map[string]string
}

type ArgvReportArgs struct {
	User     golem.Option[string]
	Password golem.Option[string]
	Mode     ArgvMode
	JSON     bool
	Text     bool
}

type ArgvEnvArgs struct {
	Token string
	Port  uint32
	Path  []string
	Debug bool
	Level uint32
}

type ArgvNumbersArgs struct {
	Value int32
	One   bool
}

type ArgvBoundedArgs struct {
	Size  uint8
	Count golem.Option[uint32]
	File  golem.Option[golem.Path]
	Site  golem.Option[golem.URL]
}

type ArgvFixture struct{}

func handle[A any](_ *tool.Context, _ A) (golem.Unit, error) { return golem.Unit{}, nil }

// argvFixture declares a fixture of the corpus as its fixtures describe it.
func argvFixture(t *testing.T, name string) reflection.Tool {
	t.Helper()
	spec := func(summary string) tool.Spec { return tool.Spec{Version: "1.0.0", Summary: summary} }
	enums := map[reflect.Type][]string{
		reflect.TypeFor[ArgvProfile](): {"debug", "release"},
		reflect.TypeFor[ArgvMode]():    {"live", "dry"},
	}
	var meta tool.Metadata
	switch name {
	case "deploy":
		meta = tool.DeclaredMetadata(t, name, spec("Deploy an artifact"), enums, func(d *tool.Definition[ArgvFixture]) {
			d.Body[ArgvDeployArgs, golem.Unit](func(a *ArgvDeployArgs, s *tool.CommandSpec) {
				s.Positional(&a.Target).Doc("Where to deploy")
				s.Positional(&a.Artifact).Doc("Artifact to deploy")
				s.Option(&a.Profile).Short('p').Default(ArgvProfileDebug).Doc("Build profile")
				s.Option(&a.Retries).Doc("Retry count")
				s.Option(&a.Offset).Doc("Start offset")
				s.Option(&a.Color).ValueName("WHEN").ValueOptional("always").Doc("Colour output")
				s.Option(&a.Output).Short('o').Aliases("out").Doc("Output file")
				s.Flag(&a.Force).Short('f').Doc("Overwrite")
				s.Flag(&a.Cache).Default(true).Negatable().Doc("Use the cache")
				s.CountFlag(&a.Verbose).Short('v').Max(3).Doc("More output")
			}).Handle(handle[ArgvDeployArgs])
		})
	case "vcs":
		meta = tool.DeclaredMetadata(t, name, spec("Version control"), enums, func(d *tool.Definition[ArgvFixture]) {
			d.Globals[ArgvVcsGlobals](func(g *ArgvVcsGlobals, s *tool.GlobalsSpec) {
				s.Option(&g.Cwd).Default("/").Doc("Working directory")
				s.Flag(&g.Quiet).Short('q').Doc("Less output")
			})
			d.Command[ArgvCommitArgs, golem.Unit]("commit", func(a *ArgvCommitArgs, s *tool.CommandSpec) {
				s.Doc("Record changes")
				s.Aliases("ci")
				s.Option(&a.Message).Short('m').Doc("Commit message")
				s.Flag(&a.Amend).Doc("Amend the last commit")
			}).Handle(handle[ArgvCommitArgs])
			d.Command[ArgvStatusArgs, golem.Unit]("status", func(a *ArgvStatusArgs, s *tool.CommandSpec) {
				s.Doc("Show the working tree state")
				s.Flag(&a.Short).Short('s').Doc("Short format")
			}).Handle(handle[ArgvStatusArgs])
		})
	case "exec":
		meta = tool.DeclaredMetadata(t, name, spec("Run a command"), enums, func(d *tool.Definition[ArgvFixture]) {
			d.Body[ArgvExecArgs, golem.Unit](func(a *ArgvExecArgs, s *tool.CommandSpec) {
				s.Tail(&a.Command).Separator("--").Verbatim().Doc("Command and its arguments")
				s.Option(&a.Cwd).Doc("Working directory")
				s.Flag(&a.DryRun).Short('n').Doc("Print only")
			}).Handle(handle[ArgvExecArgs])
		})
	case "run":
		meta = tool.DeclaredMetadata(t, name, spec("Run a program"), enums, func(d *tool.Definition[ArgvFixture]) {
			d.Body[ArgvRunArgs, golem.Unit](func(a *ArgvRunArgs, s *tool.CommandSpec) {
				s.Positional(&a.Program).Doc("Program")
				s.Tail(&a.Args).Separator("--").Max(2).Doc("Arguments")
			}).Handle(handle[ArgvRunArgs])
		})
	case "collect":
		meta = tool.DeclaredMetadata(t, name, spec("Collect values"), enums, func(d *tool.Definition[ArgvFixture]) {
			d.Body[ArgvCollectArgs, golem.Unit](func(a *ArgvCollectArgs, s *tool.CommandSpec) {
				s.List(&a.Include).Short('I').Repeated().Doc("Include path")
				s.List(&a.Tag).Delimited(',').Doc("Tag")
				s.Map(&a.Env).Short('e').Repeated().Doc("Variable")
				s.Map(&a.Label).Either(',').LastKeyWins().Doc("Label")
			}).Handle(handle[ArgvCollectArgs])
		})
	case "report":
		meta = tool.DeclaredMetadata(t, name, spec("Write a report"), enums, func(d *tool.Definition[ArgvFixture]) {
			d.Body[ArgvReportArgs, golem.Unit](func(a *ArgvReportArgs, s *tool.CommandSpec) {
				s.Option(&a.User).Doc("User")
				s.Option(&a.Password).Doc("Password")
				mode := s.Option(&a.Mode).Default(ArgvModeLive).Doc("Mode")
				s.Flag(&a.JSON).Name("json").Doc("JSON output")
				s.Flag(&a.Text).Doc("Text output")
				s.Mutex(s.Present(&a.JSON), s.Present(&a.Text))
				s.Implies(s.AllOf(s.Present(&a.User)), s.AllOf(s.Present(&a.Password)))
				s.Forbids(s.AllOf(mode.ValueIs(ArgvModeDry)), s.Present(&a.JSON))
			}).Handle(handle[ArgvReportArgs])
		})
	case "env":
		meta = tool.DeclaredMetadata(t, name, spec("Read variables"), enums, func(d *tool.Definition[ArgvFixture]) {
			d.Body[ArgvEnvArgs, golem.Unit](func(a *ArgvEnvArgs, s *tool.CommandSpec) {
				s.Option(&a.Token).Env("TOKEN").Doc("Token")
				s.Option(&a.Port).Default(80).Env("PORT").Doc("Port")
				s.List(&a.Path).Delimited(':').Env("PATHS").Doc("Search path")
				s.Flag(&a.Debug).Env("DEBUG").Doc("Debug output")
				s.CountFlag(&a.Level).Max(2).Env("LEVEL").Doc("Log level")
			}).Handle(handle[ArgvEnvArgs])
		})
	case "numbers":
		meta = tool.DeclaredMetadata(t, name, spec("Read a number"), enums, func(d *tool.Definition[ArgvFixture]) {
			d.Body[ArgvNumbersArgs, golem.Unit](func(a *ArgvNumbersArgs, s *tool.CommandSpec) {
				s.Positional(&a.Value).Doc("Value")
				s.Flag(&a.One).Short('1').Doc("Use one")
			}).Handle(handle[ArgvNumbersArgs])
		})
	case "bounded":
		meta = tool.DeclaredMetadata(t, name, spec("Read a bounded size"), enums, func(d *tool.Definition[ArgvFixture]) {
			d.Body[ArgvBoundedArgs, golem.Unit](func(a *ArgvBoundedArgs, s *tool.CommandSpec) {
				s.Positional(&a.Size).Range(1, 10).Doc("Size")
				s.Option(&a.Count).Doc("Count")
				s.Option(&a.File).Doc("File")
				s.Option(&a.Site).Doc("Site")
			}).Handle(handle[ArgvBoundedArgs])
		})
	default:
		t.Fatalf("unknown argv conformance fixture %s", name)
	}
	return reflection.ToolOf(meta)
}

type argvCase struct {
	ID        string            `json:"id"`
	Fixture   string            `json:"fixture"`
	Args      []string          `json:"args"`
	Variables map[string]string `json:"variables"`
	Expected  struct {
		Invoke      any     `json:"invoke"`
		Help        *string `json:"help"`
		Error       *string `json:"error"`
		ErrorPrefix *string `json:"errorPrefix"`
	} `json:"expected"`
}

func TestParseArgsMatchesTheArgvConformanceCorpus(t *testing.T) {
	raw, err := os.ReadFile(argvConformanceCorpus)
	if err != nil {
		t.Fatalf("the argv conformance corpus: %v", err)
	}
	var corpus struct {
		Cases []argvCase `json:"cases"`
	}
	if err := json.Unmarshal(raw, &corpus); err != nil {
		t.Fatal(err)
	}
	if len(corpus.Cases) == 0 {
		t.Fatal("the argv conformance corpus has no cases")
	}
	tools := map[string]reflection.Tool{}
	for _, c := range corpus.Cases {
		t.Run(c.ID, func(t *testing.T) {
			r, found := tools[c.Fixture]
			if !found {
				r = argvFixture(t, c.Fixture)
				tools[c.Fixture] = r
			}
			got, err := r.ParseArgsWithVariables(c.Args, c.Variables)
			want := c.Expected
			switch {
			case err != nil:
				switch {
				case want.ErrorPrefix != nil:
					if !strings.HasPrefix(err.Error(), *want.ErrorPrefix) {
						t.Errorf("error %q, want prefix %q", err, *want.ErrorPrefix)
					}
				case want.Error != nil:
					if err.Error() != *want.Error {
						t.Errorf("error %q, want %q", err, *want.Error)
					}
				default:
					t.Errorf("unexpected error %q", err)
				}
			case got.Help != "":
				if want.Help == nil || got.Help != *want.Help {
					t.Errorf("help %q, want %v", got.Help, want.Help)
				}
			default:
				data, err := json.Marshal(map[string]any{"path": append([]string{}, got.Path...), "input": got.Input})
				if err != nil {
					t.Fatal(err)
				}
				var invoke any
				_ = json.Unmarshal(data, &invoke)
				if !reflect.DeepEqual(invoke, want.Invoke) {
					t.Errorf("invoke %s, want %v", data, want.Invoke)
				}
			}
		})
	}
}
