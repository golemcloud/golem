// Package catalog is the Go consumer driven by go_mcp_import.rs: it calls a
// tool imported from an MCP server through the generated guest tool client.
package catalog

import (
	"errors"
	"fmt"

	lookup "golem.local/bridge/catalog-lookup-tool-guest-client"

	"github.com/golemcloud/golem/sdks/go/golem"
	"github.com/golemcloud/golem/sdks/go/golem/tool"
)

type ID struct{ Name string }

var Consumer = golem.DefineAgent[ID](golem.Spec{Name: "McpGoConsumer"})

type RunIn struct{ Query string }

var Run = Consumer.Method[RunIn, string]("run")

type state struct{}

func failure(err error) string {
	if message, ok := lookup.ErrMcpToolError.Match(err); ok {
		return "error:" + message
	}
	var call *tool.CallError
	if errors.As(err, &call) && call.Kind == tool.CallConstraintViolation {
		return "middleware:" + call.Message
	}
	return fmt.Sprintf("unexpected-error:%v", err)
}

func init() {
	consumer := Consumer.Implement(func(ID) *state { return &state{} })
	consumer.Handle(Run, func(_ *golem.Context[state], in RunIn) string {
		inv, err := lookup.Root.Call(func(a *lookup.RootArgs) { a.Query = in.Query })
		if err != nil {
			return failure(err)
		}
		out := inv.Collect()
		if out.Err != nil {
			return failure(out.Err)
		}
		if out.StdoutErr != nil {
			return fmt.Sprintf("stdout-error:%v", out.StdoutErr)
		}
		res := out.Result
		switch in.Query {
		case "streamed":
			if res.Structured.Answer != "stream-answer" || res.Structured.Score != 7 {
				return "bad-structured"
			}
			streamed, ok := res.Content.(lookup.ContentFieldStreamed)
			if !ok || streamed.Value.MimeType != "text/plain; charset=utf-8" {
				return "bad-stream-metadata"
			}
			if string(out.Stdout) != "finite-stream" {
				return "bad-stream-bytes"
			}
			return "streamed:stream-answer:7:finite-stream"
		case "blocks":
			if res.Structured.Answer != "blocks-answer" || res.Structured.Score != 11 {
				return "bad-structured"
			}
			blocks, ok := res.Content.(lookup.ContentFieldBlocks)
			if !ok || len(blocks.Value) != 2 {
				return "bad-blocks"
			}
			left, lok := blocks.Value[0].(lookup.BlocksFieldText)
			right, rok := blocks.Value[1].(lookup.BlocksFieldText)
			if !lok || !rok || left.Value.Text != "left" || right.Value.Text != "right" {
				return "bad-blocks"
			}
			if len(out.Stdout) != 0 {
				return "unexpected-stdout"
			}
			return "blocks:left:right:blocks-answer:11"
		}
		return "unexpected-success"
	})
}
