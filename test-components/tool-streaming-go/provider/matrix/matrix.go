// Package matrix is the Go provider of the matrix-core tool that every SDK's
// caller invokes in the tool streaming matrix.
package matrix

import (
	"slices"
	"strings"

	"github.com/golemcloud/golem/sdks/go/golem"
	"github.com/golemcloud/golem/sdks/go/golem/tool"
)

// MatrixCore is the tool's identity type.
type MatrixCore struct{}

var Tool = tool.DefineTool[MatrixCore]("matrix-core", tool.Spec{Version: "1.0.0"})

type Dimensions struct {
	Width  uint32
	Height uint32
}

type Request struct {
	Source     string
	Dimensions Dimensions
	Labels     []string
}

type InspectArgs struct {
	Request    Request
	Multiplier int64
	Caller     golem.Principal
}

// The field names match the other SDKs' providers, so every caller decodes
// the same record.
type Result struct {
	Provider         string
	Command          string
	NormalizedSource string
	WeightedSize     int64
	LabelSummary     string
	Principal        string
	OwnerAgentId     string
}

type Rejection struct {
	Field     string
	Reason    string
	Retryable bool
}

var ErrRejected = tool.DefineToolError[Rejection](Tool, "rejected",
	tool.ErrorSpec{Kind: tool.UsageError, ExitCode: 2})

var Inspect = Tool.Group("artifact").Command[InspectArgs, Result]("inspect", func(a *InspectArgs, s *tool.CommandSpec) {
	s.Positional(&a.Request)
	s.Positional(&a.Multiplier)
	s.Raises(ErrRejected)
})

func principalName(p golem.Principal) string {
	switch p := p.(type) {
	case golem.OidcPrincipal:
		return "oidc:" + p.Sub
	case golem.AgentPrincipal:
		return "agent"
	case golem.GolemUserPrincipal:
		return "golem-user"
	default:
		return "anonymous"
	}
}

var _ = Inspect.Handle(func(_ *tool.Context, a InspectArgs) (Result, error) {
	if a.Request.Source == "reject.me" {
		return Result{}, ErrRejected.New(Rejection{Field: "request.source", Reason: "unsupported source"})
	}
	labels := slices.Clone(a.Request.Labels)
	slices.Reverse(labels)
	return Result{
		Provider:         "go",
		Command:          "artifact/inspect",
		NormalizedSource: strings.ToUpper(a.Request.Source),
		WeightedSize:     int64(a.Request.Dimensions.Width)*int64(a.Request.Dimensions.Height)*a.Multiplier + int64(len(a.Request.Labels)),
		LabelSummary:     strings.Join(labels, "|"),
		Principal:        principalName(a.Caller),
		OwnerAgentId:     golem.MustGetSelfMetadata().AgentID.AgentID,
	}, nil
})
