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

package gol40_test

// The rich conformance tool of test-data/gol-40/rich-tool-conformance-v1.json,
// declared with the public API. The CLI's metadata conformance test builds this
// file, up to the native checks below, into a component and compares what the
// component publishes with the contract.

import (
	"io"
	"strings"
	"testing"

	"github.com/golemcloud/golem/sdks/go/golem"
	"github.com/golemcloud/golem/sdks/go/golem/tool"
)

type Region uint8

const (
	RegionEuWest1 Region = iota
	RegionUsEast1
)

var _ = golem.DefineEnum[Region]("eu-west-1", "us-east-1")

type Profile uint8

const (
	ProfileDebug Profile = iota
	ProfileRelease
)

var _ = golem.DefineEnum[Profile]("debug", "release")

type ReportFormat uint8

const (
	ReportFormatJSON ReportFormat = iota
	ReportFormatText
)

var _ = golem.DefineEnum[ReportFormat]("json", "text")

type ColorMode uint8

const (
	ColorModeAuto ColorMode = iota
	ColorModeAlways
	ColorModeNever
)

var _ = golem.DefineEnum[ColorMode]("auto", "always", "never")

type RenderStatus uint8

const (
	RenderStatusQueued RenderStatus = iota
	RenderStatusReady
	RenderStatusFailed
)

var _ = golem.DefineEnum[RenderStatus]("queued", "ready", "failed")

type ArtifactRequest struct {
	Source string
	Labels map[string]string
}

type ArtifactReport struct {
	ArtifactId uint64
	Digest     golem.Text `golem:"minLength=8,maxLength=64,regex=^[a-f0-9]+$"`
	Labels     map[string]string
	Warnings   []string
}

type ValidationFailure struct {
	Field     string
	Reason    string
	Retryable bool
}

type Artifact struct{}

var ArtifactTool = tool.DefineTool[Artifact]("artifact", tool.Spec{
	Version:     "1.0.0",
	Summary:     "Build and inspect artifacts",
	Description: "A deliberately asymmetric conformance tool.",
	Aliases:     []string{"art"},
})

var ErrInvalidRequest = tool.DefineToolError[ValidationFailure](ArtifactTool, "invalid-request", tool.ErrorSpec{
	Kind: tool.UsageError, ExitCode: 2, Summary: "Request validation failed",
})

var ErrRenderFailed = tool.DefineToolError[struct {
	Stage string
	Code  uint32
}](ArtifactTool, "render-failed", tool.ErrorSpec{
	Kind: tool.RuntimeError, ExitCode: 70, Summary: "Renderer failed",
})

type ArtifactGlobals struct {
	Region Region
	Trace  bool
}

var _ = ArtifactTool.
	Example("Render", "artifact --region eu-west-1 render src/main.wasm --format json").
	Globals[ArtifactGlobals](func(g *ArtifactGlobals, s *tool.GlobalsSpec) {
	s.Option(&g.Region).Short('r').Aliases("location").ValueName("REGION").
		Default(RegionEuWest1).Env("ARTIFACT_REGION").
		Doc("Execution region").Description("Inherited by every executable descendant.")
	s.Flag(&g.Trace).Short('t').Aliases("diagnostics").Negatable().Doc("Emit trace details")
})

type RenderGlobals struct {
	Profile Profile
}

var render = ArtifactTool.Group("render").
	Aliases("build").
	Doc("Render one artifact").
	Description("Build an artifact and return a structured report.").
	Example("Release build", "artifact render src/main.wasm --format json --tag release --define opt=3 --checksum").
	Globals[RenderGlobals](func(g *RenderGlobals, s *tool.GlobalsSpec) {
	s.Option(&g.Profile).Short('p').ValueName("PROFILE").Default(ProfileRelease).
		Doc("Build profile").Description("Inherited by render descendants.")
})

type RenderArgs struct {
	ArtifactGlobals
	RenderGlobals
	Request  ArtifactRequest
	Inputs   []golem.Path
	Format   ReportFormat
	Tag      []string
	Define   map[string]int64
	Color    ColorMode
	Checksum bool
	Verbose  uint32
	Module   io.Reader
}

var Render = render.OutputBody[RenderArgs, ArtifactReport](func(a *RenderArgs, s *tool.CommandSpec) {
	s.Positional(&a.Request).ValueName("REQUEST").Doc("Artifact request")
	s.Tail(&a.Inputs).ValueName("INPUT").Min(1).Max(3).Separator("--").Verbatim().
		Direction(golem.PathInput).PathKind(golem.PathFile).Extensions("wasm", "wat").
		Doc("Input modules").Description("One to three source modules.")
	format := s.Option(&a.Format).Short('f').Aliases("output-format").ValueName("FORMAT").
		Default(ReportFormatJSON).Doc("Report format")
	tag := s.List(&a.Tag).Aliases("label").ValueName("TAG").Either(',').Default(nil).
		Doc("Tags").Description("May be repeated or comma-delimited.")
	define := s.Map(&a.Define).Short('D').ValueName("KEY=VALUE").Default(map[string]int64{}).
		Doc("Numeric definitions").Description("Duplicate keys are rejected.")
	s.Option(&a.Color).ValueName("WHEN").ValueOptional(ColorModeAuto).
		Doc("Color mode").Description("Bare presence resolves to the default.")
	checksum := s.Flag(&a.Checksum).Short('c').Aliases("digest").Negatable().Doc("Include digest")
	s.CountFlag(&a.Verbose).Short('v').Max(3).Doc("Verbosity")

	s.RequiresAll(checksum, format)
	s.Implies(s.AllOf(s.ValueIs(&a.Profile, ProfileRelease)), s.AnyOf(tag))
	s.Forbids(s.AnyOf(format.ValueIs(ReportFormatText)), define)

	s.Stdin(&a.Module).Optional().Mime("application/wasm").Doc("Optional module bytes")
	s.Stdout().Required().Mime("text/plain; charset=utf-8").Doc("Progress output")
	s.Stderr().Mime("application/octet-stream").Doc("Diagnostic bytes")
	s.ResultDoc("Artifact report")
	s.Formatter("json", "JSON report")
	s.Formatter("table", "Tabular report")
	s.DefaultFormatter("json")
	s.Raises(ErrInvalidRequest, ErrRenderFailed)
	s.Idempotent()
})

var _ = Render.Handle(func(_ *tool.OutputContext, a RenderArgs) (ArtifactReport, error) {
	return ArtifactReport{Labels: a.Request.Labels}, nil
})

type StatusArgs struct {
	ArtifactGlobals
	RenderGlobals
	ArtifactId uint64
}

var Status = render.Command[StatusArgs, RenderStatus]("status", func(a *StatusArgs, s *tool.CommandSpec) {
	s.Aliases("show")
	s.Doc("Inspect render status")
	s.Positional(&a.ArtifactId).ValueName("ID").Doc("Artifact identifier")
	s.ResultDoc("Current status")
	s.Formatter("json", "JSON status")
	s.DefaultFormatter("json")
	s.ReadOnly()
	s.Idempotent()
})

var _ = Status.Handle(func(_ *tool.Context, _ StatusArgs) (RenderStatus, error) {
	return RenderStatusReady, nil
})

// Native checks.

func TestGol40RichFixtureDefinesCleanly(t *testing.T) {
	for _, err := range golem.DefinitionErrors() {
		if strings.Contains(err.Error(), "artifact") {
			t.Errorf("definition error: %v", err)
		}
	}
}
