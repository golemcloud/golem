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
	"strings"
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

	// A case of the same name with another payload type fails to match, and
	// leaves the payload for the right one.
	type otherPayload struct {
		Card   golem.PermissionCard
		Number uint32
	}
	other := &ErrorCase[otherPayload, struct{}]{info: &toolErrorInfo{tool: "keys", name: "locked", payload: reflect.TypeFor[otherPayload]()}}
	if _, ok := other.Match(err); ok {
		t.Fatal("a case with another payload type matched")
	}

	first, ok := locked.Match(err)
	if !ok {
		t.Fatal("the first Match failed")
	}
	if _, ok := other.Match(err); ok {
		t.Error("a case with another payload type matched after the right one")
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
	if _, err := ctx.Next(ctx.Input()); !errors.Is(err, golem.ErrPermissionCardTaken) || started {
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

type TwoCards struct{ A, B golem.PermissionCard }

// partlyTaken is a TwoCards value whose first card a decode took and whose
// second card is still a live host handle.
func partlyTaken(t *testing.T) (golem.TypedValue, *types.PermissionCard) {
	t.Helper()
	g := engine.GraphBuilder{E: link.Engine}
	root := g.Node(link.Engine.Compile(reflect.TypeFor[TwoCards]()))
	graph := g.Build()
	graph.Root = root
	live := &types.PermissionCard{}
	return link.TypedValue(types.TypedSchemaValue{Graph: graph, Value: types.SchemaValueTree{
		ValueNodes: []types.SchemaValueNode{
			types.MakeSchemaValueNodePermissionCardHandle(nil),
			types.MakeSchemaValueNodePermissionCardHandle(live),
			types.MakeSchemaValueNodeRecordValue([]int32{0, 1}),
		},
		Root: 2,
	}}).(golem.TypedValue), live
}

func recordReleases(t *testing.T) *[]*types.PermissionCard {
	t.Helper()
	var released []*types.PermissionCard
	prev := link.ReleaseCard
	t.Cleanup(func() { link.ReleaseCard = prev })
	link.ReleaseCard = func(h *types.PermissionCard) { released = append(released, h) }
	return &released
}

// A declared error whose payload card a Match took cannot pass on whole: it
// becomes an invalid result, and the payload's other cards go back.
func TestADeclaredErrorWhosePayloadWasMatchedIsNotPassedOn(t *testing.T) {
	released := recordReleases(t)
	payload, live := partlyTaken(t)
	err := toolCallErrorFromWit("keys", []string{"get"}, types.MakeToolRpcErrorRemoteToolError(
		types.MakeToolErrorCustomError(types.CustomToolError{Name: "locked", Payload: witOf(payload)})))
	got := passThroughToolError(fmt.Errorf("handler: %w", err))
	if got.Tag() != types.ToolErrorInvalidResult || !strings.Contains(got.InvalidResult(), "locked") {
		t.Errorf("the matched error passed on as %+v", got)
	}
	if len(*released) != 1 || (*released)[0] != live {
		t.Errorf("released %v, want the payload's live card", *released)
	}

	whole := hostValue(t, func(card func() golem.PermissionCard) CardPayload { return CardPayload{Card: card()} })
	intact := toolCallErrorFromWit("keys", nil, types.MakeToolRpcErrorRemoteToolError(
		types.MakeToolErrorCustomError(types.CustomToolError{Name: "locked", Payload: witOf(whole)})))
	if got := passThroughToolError(intact); got.Tag() != types.ToolErrorCustomError {
		t.Errorf("an unmatched error passed on as %+v", got)
	}
}

// A value handed on once moved its cards to the layer beneath: handing it on
// again fails rather than name handles the host already owns.
func TestAnInputIsHandedOnOnce(t *testing.T) {
	input := hostValue(t, func(card func() golem.PermissionCard) KeysArgs { return KeysArgs{Card: card()} })
	starts := 0
	inv := &middlewareInvocation{toolName: "keys", input: witOf(input), under: underlyingLayer{start: func([]string, types.TypedSchemaValue, io.Reader) (toolCall, error) {
		starts++
		return toolCall{wait: func() (witTypes.Option[types.TypedSchemaValue], *types.ToolRpcError) {
			return witTypes.None[types.TypedSchemaValue](), nil
		}, cancel: func() {}}, nil
	}}}
	ctx := &UniversalMiddlewareContext[golem.Unit]{inv: inv}
	if _, err := ctx.Next(ctx.Input()); err != nil {
		t.Fatal(err)
	}
	if _, err := ctx.Next(ctx.Input()); !errors.Is(err, golem.ErrPermissionCardTaken) || starts != 1 {
		t.Errorf("handing the input on again gave %v after %d starts", err, starts)
	}
}

// A universal middleware's result that cannot be sent fails the invocation
// and gives its remaining cards back.
func TestARejectedMiddlewareResultReleasesItsCards(t *testing.T) {
	released := recordReleases(t)
	v, r, d := newVcs(t)
	result, live := partlyTaken(t)
	m := defineUniversalToolMiddlewareInto[golem.Unit](r, d, "keeper", MiddlewareSpec{})
	_ = m.Handle(func(*UniversalMiddlewareContext[golem.Unit]) (golem.Option[golem.TypedValue], error) {
		return golem.Some(result), nil
	})
	input := encodeArgs(t, v.commit.ce, func(a *CommitArgs) { a.Message = "m" })
	res, _ := runMiddlewareFor(t, r, d, "keeper", middlewareRun{path: []string{"commit"}, input: input, params: mustTypedValue(t, golem.Unit{})})
	if res.IsOk() || res.Err().Tag() != types.ToolErrorInvalidResult {
		t.Errorf("the result passed as %+v", res)
	}
	if len(*released) != 1 || (*released)[0] != live {
		t.Errorf("released %v, want the result's live card", *released)
	}
}

// An input record of the wrong shape gives its cards back, though the decode
// never reached them.
func TestAnInputOfTheWrongShapeReleasesItsCards(t *testing.T) {
	released := recordReleases(t)
	cmd, _, _ := declareKeys(t)
	l, _ := cmd.ce.resolve()
	live := &types.PermissionCard{}
	tree := types.SchemaValueTree{ValueNodes: []types.SchemaValueNode{
		types.MakeSchemaValueNodePermissionCardHandle(live),
		types.MakeSchemaValueNodeStringValue("extra"),
		types.MakeSchemaValueNodeRecordValue([]int32{0, 1}),
	}, Root: 2}
	if err := l.decode(tree, reflect.New(cmd.ce.argsType).Elem()); err == nil {
		t.Fatal("a record of two fields decoded as one")
	}
	if len(*released) != 1 || (*released)[0] != live {
		t.Errorf("released %v, want the record's card", *released)
	}
}
