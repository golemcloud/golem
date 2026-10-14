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

package tool

import (
	"errors"
	"fmt"
	"io"
	"reflect"
	"testing"

	"github.com/golemcloud/golem/sdks/go/golem"
	"github.com/golemcloud/golem/sdks/go/golem/internal/engine"
	"github.com/golemcloud/golem/sdks/go/golem/internal/link"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// hostValue is a value as the host hands one over: T's schema, and a tree in
// which each card is a fresh host handle.
func hostValue[T any](t *testing.T, build func(card func() golem.PermissionCard) T) golem.TypedValue {
	t.Helper()
	card := func() golem.PermissionCard {
		g := engine.GraphBuilder{E: link.Engine}
		root := g.Node(link.Engine.Compile(reflect.TypeFor[golem.PermissionCard]()))
		graph := g.Build()
		graph.Root = root
		wire := link.TypedValue(types.TypedSchemaValue{Graph: graph, Value: types.SchemaValueTree{
			ValueNodes: []types.SchemaValueNode{types.MakeSchemaValueNodePermissionCardHandle(&types.PermissionCard{})},
		}}).(golem.TypedValue)
		c, err := golem.DecodeTypedValue[golem.PermissionCard](wire)
		if err != nil {
			t.Fatal(err)
		}
		return c
	}
	return golem.EncodeTypedValue(build(card))
}

// held reports whether card still holds its handle. It moves the card, so it
// is a test's last use of it.
func held(card golem.PermissionCard) (ok bool) {
	defer func() { ok = recover() == nil }()
	golem.EncodeTypedValue(card)
	return true
}

type CardPayload struct{ Card golem.PermissionCard }

// Match decodes a declared error's payload once: every later Match, on the
// error or a copy of it, returns the same card.
func TestMatchDecodesACardPayloadOnce(t *testing.T) {
	payload := hostValue(t, func(card func() golem.PermissionCard) CardPayload { return CardPayload{Card: card()} })
	err := toolCallErrorFromWit("keys", []string{"get"}, types.MakeToolRpcErrorRemoteToolError(
		types.MakeToolErrorCustomError(types.CustomToolError{Name: "locked", Payload: witOf(payload)})))
	locked := &ErrorCase[CardPayload, struct{}]{info: &toolErrorInfo{tool: "keys", name: "locked", payload: reflect.TypeFor[CardPayload]()}}

	first, ok := locked.Match(err)
	if !ok {
		t.Fatal("the first Match failed")
	}
	copied := *err
	for name, e := range map[string]error{"the error": err, "a copy": &copied, "a wrapper": fmt.Errorf("wrapped: %w", err)} {
		if again, ok := locked.Match(e); !ok || again != first {
			t.Errorf("Match on %s gave %v, %v", name, again, ok)
		}
	}
}

type KeysArgs struct{ Card golem.PermissionCard }

type Keys struct{}

func declareKeys(t *testing.T) (*Command[Keys, KeysArgs, golem.Unit], *toolRegistry, *definitions) {
	t.Helper()
	r, d := newToolRegistry(), newDefinitions()
	def := defineToolInto[Keys](r, d, "keys", Spec{Version: "1.0.0"}, false)
	cmd := def.Body[KeysArgs, golem.Unit](func(a *KeysArgs, s *CommandSpec) { s.Positional(&a.Card) })
	_ = cmd.Handle(func(*Context, KeysArgs) (golem.Unit, error) { return golem.Unit{}, nil })
	if _, ok := r.discover(d); !ok {
		t.Fatalf("tool discovery failed: %s", engine.AllErrors(d.Errs))
	}
	return cmd, r, d
}

// A call the host refuses to start took nothing: the card in the arguments
// goes back to the caller.
func TestAToolCallThatFailsToStartGivesItsCardsBack(t *testing.T) {
	cmd, _, _ := declareKeys(t)
	prev := startToolCall
	t.Cleanup(func() { startToolCall = prev })
	startToolCall = func(string, []string, types.TypedSchemaValue, io.Reader, Streams) (toolCall, error) {
		return toolCall{}, errors.New("no such tool")
	}
	card := hostValue(t, func(card func() golem.PermissionCard) KeysArgs { return KeysArgs{Card: card()} })
	args, _ := golem.DecodeTypedValue[KeysArgs](card)
	if _, err := cmd.Call(func(a *KeysArgs) { a.Card = args.Card }); err == nil {
		t.Fatal("the call started")
	}
	if !held(args.Card) {
		t.Error("the card stayed moved after the call failed to start")
	}
}

// The same holds for a middleware's call of the command beneath, when the
// layer beneath refuses to start it.
func TestAnUnderlyingCallThatFailsToStartGivesItsCardsBack(t *testing.T) {
	cmd, _, _ := declareKeys(t)
	entry := cmd.ce.node.entry
	inv := &middlewareInvocation{toolName: "keys", under: absentUnderlyingLayer}
	args, _ := golem.DecodeTypedValue[KeysArgs](hostValue(t, func(card func() golem.PermissionCard) KeysArgs { return KeysArgs{Card: card()} }))
	if _, err := startUnderlying[golem.Unit](cmd.ce, inv, &middlewareEntry{name: "m", wrapped: entry}, fillArgs(func(a *KeysArgs) { a.Card = args.Card })); err == nil {
		t.Fatal("the call started")
	}
	if !held(args.Card) {
		t.Error("the card stayed moved after the layer beneath refused the call")
	}
}

// An input whose card the middleware decoded out cannot be handed on whole.
func TestAnInputWhoseCardWasDecodedIsNotHandedOn(t *testing.T) {
	input := hostValue(t, func(card func() golem.PermissionCard) KeysArgs { return KeysArgs{Card: card()} })
	started := false
	inv := &middlewareInvocation{toolName: "keys", under: underlyingLayer{start: func([]string, types.TypedSchemaValue, io.Reader) (toolCall, error) {
		started = true
		return toolCall{}, errors.New("unexpected")
	}}}
	ctx := &UniversalMiddlewareContext[golem.Unit]{inv: inv}
	inv.input = witOf(input)
	if _, err := golem.DecodeTypedValue[KeysArgs](ctx.Input()); err != nil {
		t.Fatal(err)
	}
	if _, err := ctx.Next(ctx.Input()); !errors.Is(err, engine.ErrPermissionCardTaken) || started {
		t.Errorf("handing on the input gave %v, started %v", err, started)
	}
}

type KeysResult struct {
	Card   golem.PermissionCard
	Number uint32
}

// A result that does not decode gives its cards back to the host.
func TestAToolResultThatDoesNotDecodeReleasesItsCards(t *testing.T) {
	cmd, _, _ := declareKeys(t)
	var released []*types.PermissionCard
	prev := link.ReleaseCard
	t.Cleanup(func() { link.ReleaseCard = prev })
	link.ReleaseCard = func(h *types.PermissionCard) { released = append(released, h) }

	type wrongResult struct {
		Card golem.PermissionCard
		Text string
	}
	result := hostValue(t, func(card func() golem.PermissionCard) wrongResult { return wrongResult{Card: card(), Text: "t"} })
	h := witOf(result).Value.ValueNodes
	var handle *types.PermissionCard
	for _, n := range h {
		if n.Tag() == types.SchemaValueNodePermissionCardHandle {
			handle = n.PermissionCardHandle()
		}
	}
	ce := &commandEntry{node: cmd.ce.node, outType: reflect.TypeFor[KeysResult]()}
	_, err := ce.finish("keys", toolCall{wait: func() (witTypes.Option[types.TypedSchemaValue], *types.ToolRpcError) {
		return witTypes.Some(witOf(result)), nil
	}})
	if err == nil {
		t.Fatal("a mismatched result decoded")
	}
	if len(released) != 1 || released[0] != handle {
		t.Errorf("released %v, want the result's card", released)
	}
}
