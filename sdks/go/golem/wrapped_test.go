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

package golem

import (
	"reflect"
	"testing"
	"time"

	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
)

// Each case wraps a payload that a defined type would change: `type EventAt
// time.Time` is not a time.Time to the SDK, `type EventNote Text` is a plain
// string, and a defined type over an Option or an interface loses what makes
// it one.
type Event interface{ isEvent() }

type EventAt struct{ Value time.Time }
type EventNote struct{ Value Text }
type EventMaybe struct{ Value Option[string] }
type EventPaid struct{ Value PaymentMethod }
type EventCount struct{ N int64 }
type EventCleared struct{}

func (EventAt) isEvent()      {}
func (EventNote) isEvent()    {}
func (EventMaybe) isEvent()   {}
func (EventPaid) isEvent()    {}
func (EventCount) isEvent()   {}
func (EventCleared) isEvent() {}

var _ = DefineVariant[Event](
	WrappedCase[EventAt]("at"),
	WrappedCase[EventNote]("note"),
	WrappedCase[EventMaybe]("maybe"),
	WrappedCase[EventPaid]("paid"),
	Case[EventCount]("count"),
	Case[EventCleared]("cleared"),
)

// The payload each case publishes is its field's schema, so the distinctions a
// defined type would erase survive: datetime, text, option, a nested variant.
func TestAWrappedCasePublishesItsFieldsSchema(t *testing.T) {
	g := graphBuilder{d: defs}
	root := g.node(defs.compile(reflect.TypeFor[Event]()))
	graph := g.build()

	want := map[string]uint8{
		"at":    types.SchemaTypeBodyDatetimeType,
		"note":  types.SchemaTypeBodyTextType,
		"maybe": types.SchemaTypeBodyOptionType,
		"paid":  types.SchemaTypeBodyVariantType,
		// Case, not WrappedCase: the struct is the payload, so a record.
		"count": types.SchemaTypeBodyRecordType,
	}
	for _, c := range graph.TypeNodes[root].Body.VariantType() {
		if c.Name == "cleared" {
			if c.Payload.IsSome() {
				t.Fatalf("cleared is an empty struct and carries no payload")
			}
			continue
		}
		if c.Payload.IsNone() {
			t.Fatalf("case %q lost its payload", c.Name)
		}
		got := resolveBody(graph, c.Payload.Some()).Tag()
		if got != want[c.Name] {
			t.Fatalf("case %q publishes tag %d, want %d", c.Name, got, want[c.Name])
		}
	}
}

// resolveBody follows a named-type reference to its body, since a nested
// variant is published once as a definition and referred to from the case.
func resolveBody(g types.SchemaGraph, idx int32) types.SchemaTypeBody {
	body := g.TypeNodes[idx].Body
	for body.Tag() == types.SchemaTypeBodyRefType {
		body = g.TypeNodes[g.Defs[body.RefType()].Body].Body
	}
	return body
}

func TestWrappedCasesRoundTrip(t *testing.T) {
	at := time.Date(2026, 9, 24, 10, 30, 0, 0, time.UTC)
	assertRoundTrip(t, "datetime", Event(EventAt{Value: at}))
	assertRoundTrip(t, "text", Event(EventNote{Value: "hello"}))
	assertRoundTrip(t, "option, some", Event(EventMaybe{Value: Some("x")}))
	assertRoundTrip(t, "option, none", Event(EventMaybe{Value: None[string]()}))
	assertRoundTrip(t, "nested variant", Event(EventPaid{Value: Card{Number: "1", Expiry: StatusActive}}))
	assertRoundTrip(t, "nested payloadless case", Event(EventPaid{Value: Cash{}}))
	assertRoundTrip(t, "plain case", Event(EventCount{N: 7}))
	assertRoundTrip(t, "payloadless case", Event(EventCleared{}))
}

type wrapTwo struct{ A, B string }
type wrapPrivate struct{ v string } //nolint:unused // present so the wrapper has an unexported field to reject
type wrapNotStruct string
type wrapBad interface{ isWrapBad() }

func (wrapTwo) isWrapBad()       {}
func (wrapPrivate) isWrapBad()   {}
func (wrapNotStruct) isWrapBad() {}

// A wrapper that does not have exactly one exported field leaves it unclear
// which part is the payload, so it is a definition error rather than a guess.
func TestAMalformedWrapperIsRejected(t *testing.T) {
	withDefs(t, func(d *definitions) {
		defineVariantInto[wrapBad](d,
			WrappedCase[wrapTwo]("two"),
			WrappedCase[wrapPrivate]("private"),
			WrappedCase[wrapNotStruct]("scalar"),
		)
		mustDefErr(t, d, "has 2 fields; a wrapper has exactly one")
		mustDefErr(t, d, "is unexported")
		mustDefErr(t, d, "is not a struct")
	})
}

// A union branch can wrap its body for the same reason: a golem.Text cannot
// carry the union's marker method without becoming a plain string.
type Handle interface{ isHandle() }

type HandleUser struct{ Value Text }
type HandleTeam struct{ Value Text }

func (HandleUser) isHandle() {}
func (HandleTeam) isHandle() {}

var _ = DefineUnion[Handle](
	WrappedBranch[HandleUser]("user", Prefix("@")),
	WrappedBranch[HandleTeam]("team", Prefix("#")),
)

func TestAWrappedBranchPublishesAndRoundTripsItsBody(t *testing.T) {
	g := graphBuilder{d: defs}
	root := g.node(defs.compile(reflect.TypeFor[Handle]()))
	graph := g.build()
	for _, b := range graph.TypeNodes[root].Body.UnionType().Branches {
		if tag := resolveBody(graph, b.Body).Tag(); tag != types.SchemaTypeBodyTextType {
			t.Fatalf("branch %q publishes tag %d, want text", b.Tag, tag)
		}
	}
	assertRoundTrip(t, "user", Handle(HandleUser{Value: "@ada"}))
	assertRoundTrip(t, "team", Handle(HandleTeam{Value: "#golem"}))
}
