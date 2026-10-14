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
	"encoding/json"
	"errors"
	"fmt"
	"go/ast"
	"go/parser"
	"go/token"
	"os"
	"reflect"
	"regexp"
	"strings"
	"testing"
	"time"

	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	perm "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_permissions_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// fakeCards is a host that knows a fixed set of handles.
type fakeCards struct {
	ids      map[*types.PermissionCard]types.CardId
	parents  map[*types.PermissionCard][]types.CardId
	released []*types.PermissionCard
	consumed []*types.PermissionCard
	derived  [][]perm.PatternGrant
	lifespan witTypes.Option[perm.Duration]
	scope    []*types.PermissionCard
	target   perm.Holder
	err      witTypes.Option[perm.PermissionError]
	next     *types.PermissionCard
}

func newFakeCards(t *testing.T) *fakeCards {
	f := &fakeCards{
		ids:     map[*types.PermissionCard]types.CardId{},
		parents: map[*types.PermissionCard][]types.CardId{},
		err:     witTypes.None[perm.PermissionError](),
	}
	prev := permissionsHostOps
	t.Cleanup(func() { permissionsHostOps = prev })
	cardResult := func() witTypes.Result[*types.PermissionCard, perm.PermissionError] {
		if f.err.IsSome() {
			return witTypes.Err[*types.PermissionCard](f.err.Some())
		}
		return witTypes.Ok[*types.PermissionCard, perm.PermissionError](f.next)
	}
	permissionsHostOps = permissionsHost{
		id:      func(c *types.PermissionCard) types.CardId { return f.ids[c] },
		parents: func(c *types.PermissionCard) []types.CardId { return f.parents[c] },
		expiresAt: func(*types.PermissionCard) witTypes.Option[perm.Timestamp] {
			return witTypes.Some(perm.Timestamp{Seconds: 1_700_000_000, Nanoseconds: 5})
		},
		inspect: func(*types.PermissionCard) witTypes.Result[perm.CardView, perm.PermissionError] {
			if f.err.IsSome() {
				return witTypes.Err[perm.CardView](f.err.Some())
			}
			return witTypes.Ok[perm.CardView, perm.PermissionError](perm.CardView{
				LowerPositive: []perm.PatternGrant{{Class: "kv", Owner: "o", Recipient: "r", Verb: "read", ResourceId: "a"}},
				UpperNegative: []perm.PatternGrant{{Class: "blob", Verb: "write", ResourceId: "b"}},
			})
		},
		derive: func(parent *types.PermissionCard, lp, ln, up, un []perm.PatternGrant, lifespan witTypes.Option[perm.Duration]) witTypes.Result[*types.PermissionCard, perm.PermissionError] {
			f.scope = []*types.PermissionCard{parent}
			f.derived = [][]perm.PatternGrant{lp, ln, up, un}
			f.lifespan = lifespan
			return cardResult()
		},
		deriveFromWallet: func(lp, ln, up, un []perm.PatternGrant, lifespan witTypes.Option[perm.Duration]) witTypes.Result[*types.PermissionCard, perm.PermissionError] {
			f.derived = [][]perm.PatternGrant{lp, ln, up, un}
			f.lifespan = lifespan
			return cardResult()
		},
		deriveScope: func(parents []*types.PermissionCard, lp, ln, up, un []perm.PatternGrant) witTypes.Result[*types.PermissionCard, perm.PermissionError] {
			f.scope = parents
			f.derived = [][]perm.PatternGrant{lp, ln, up, un}
			return cardResult()
		},
		revoke: func(c *types.PermissionCard) witTypes.Result[uint32, perm.PermissionError] {
			f.consumed = append(f.consumed, c)
			if f.err.IsSome() {
				return witTypes.Err[uint32](f.err.Some())
			}
			return witTypes.Ok[uint32, perm.PermissionError](3)
		},
		install: func(c *types.PermissionCard, target perm.Holder) witTypes.Result[witTypes.Unit, perm.PermissionError] {
			f.consumed = append(f.consumed, c)
			f.target = target
			if f.err.IsSome() {
				return witTypes.Err[witTypes.Unit](f.err.Some())
			}
			return witTypes.Ok[witTypes.Unit, perm.PermissionError](witTypes.Unit{})
		},
		wallet: func() []*types.PermissionCard {
			return []*types.PermissionCard{{}, {}}
		},
		walletVersion: func() perm.WalletVersionToken {
			return perm.WalletVersionToken{WalletIdHash: []uint8{1, 2}, Generation: 7}
		},
		listModules: func() []KernelModule { return []KernelModule{{ClassName: "kv", Verbs: []string{"read"}}} },
		validateGrant: func(perm.PatternGrant) witTypes.Result[witTypes.Unit, perm.PermissionError] {
			if f.err.IsSome() {
				return witTypes.Err[witTypes.Unit](f.err.Some())
			}
			return witTypes.Ok[witTypes.Unit, perm.PermissionError](witTypes.Unit{})
		},
		release: func(c *types.PermissionCard) { f.released = append(f.released, c) },
	}
	return f
}

func (f *fakeCards) card(hi, lo uint64) PermissionCard {
	h := &types.PermissionCard{}
	f.ids[h] = types.CardId{Uuid: types.Uuid{HighBits: hi, LowBits: lo}}
	return newPermissionCard(h)
}

func (f *fakeCards) fail(e perm.PermissionError) { f.err = witTypes.Some(e) }

func (*fakeCards) ops() *permissionsHost { return &permissionsHostOps }

func mustPanicMoved(t *testing.T, what string, f func()) {
	t.Helper()
	defer func() {
		t.Helper()
		if r := recover(); !errors.Is(asError(r), ErrPermissionCardMoved) {
			t.Errorf("%s on a moved card gave %v", what, r)
		}
	}()
	f()
}

// The kinds follow the WIT variant: the same cases, in the same order, under
// the same names. A case added to the WIT fails here before it can reach the
// converter's panic.
func TestPermissionErrorKindsAreTheWitCases(t *testing.T) {
	wit, err := os.ReadFile("wit/deps/golem-permissions/types.wit")
	if err != nil {
		t.Fatal(err)
	}
	body := regexp.MustCompile(`(?s)variant permission-error \{(.*?)\}`).FindSubmatch(wit)
	if body == nil {
		t.Fatal("no permission-error variant in the WIT")
	}
	var cases []string
	for _, m := range regexp.MustCompile(`([a-z-]+)\(string\)`).FindAllSubmatch(body[1], -1) {
		cases = append(cases, string(m[1]))
	}
	if !reflect.DeepEqual(cases, permissionErrorKindNames[:]) {
		t.Fatalf("the WIT declares %v, the SDK pins %v", cases, permissionErrorKindNames)
	}
	if got := permissionTagCount(t); got != permissionErrorKindCount {
		t.Fatalf("the bindings declare %d permission-error tags, the SDK pins %d", got, permissionErrorKindCount)
	}

	makers := []func(string) perm.PermissionError{
		perm.MakePermissionErrorNotPermitted,
		perm.MakePermissionErrorLowerBoundTooBroad,
		perm.MakePermissionErrorUpperBoundTooBroad,
		perm.MakePermissionErrorWalletMultiSourceRequired,
		perm.MakePermissionErrorCardRevoked,
		perm.MakePermissionErrorCardExpired,
		perm.MakePermissionErrorInstallRecipientMismatch,
		perm.MakePermissionErrorUnknownResourceClass,
		perm.MakePermissionErrorInvalidPattern,
		perm.MakePermissionErrorInvalidVerb,
		perm.MakePermissionErrorInvalidOwner,
		perm.MakePermissionErrorInvalidRecipient,
	}
	if len(makers) != permissionErrorKindCount {
		t.Fatalf("%d makers for %d kinds", len(makers), permissionErrorKindCount)
	}
	for i, mk := range makers {
		msg := fmt.Sprintf("message %d", i)
		var pe *PermissionError
		if !errors.As(permissionErrorFromWit(mk(msg)), &pe) {
			t.Fatalf("case %d is not a *PermissionError", i)
		}
		if int(pe.Kind) != i || pe.Message != msg || pe.Kind.String() != cases[i] {
			t.Errorf("case %s became %v %q", cases[i], pe.Kind, pe.Message)
		}
	}
	if PermissionInstallRecipientMismatch.String() != "install-recipient-mismatch" {
		t.Error("the kind constants drifted from the case order")
	}
}

func permissionTagCount(t *testing.T) int {
	t.Helper()
	file, err := parser.ParseFile(token.NewFileSet(), "internal/wit/golem_permissions_types/wit_bindings.go", nil, 0)
	if err != nil {
		t.Fatal(err)
	}
	n := 0
	ast.Inspect(file, func(node ast.Node) bool {
		if vs, ok := node.(*ast.ValueSpec); ok {
			for _, name := range vs.Names {
				if strings.HasPrefix(name.Name, "PermissionError") {
					n++
				}
			}
		}
		return true
	})
	return n
}

func TestDerivePassesTheBoundsInWitOrder(t *testing.T) {
	f := newFakeCards(t)
	parent := f.card(1, 2)
	f.next = &types.PermissionCard{}
	child, err := parent.Derive(Bounds{
		LowerPositive: []PatternGrant{{Class: "lp"}},
		LowerNegative: []PatternGrant{{Class: "ln"}},
		UpperPositive: []PatternGrant{{Class: "up"}},
		UpperNegative: []PatternGrant{{Class: "un", Owner: "o", Recipient: "r", Verb: "v", ResourceID: "id"}},
	}, Some(LifespanOf(90*time.Minute)))
	if err != nil {
		t.Fatal(err)
	}
	if child.st.h != f.next || f.scope[0] != parent.st.h {
		t.Fatal("the child does not wrap the derived handle, or the parent was not borrowed")
	}
	if parent.st.h == nil {
		t.Fatal("deriving moved the parent")
	}
	var got []string
	for _, l := range f.derived {
		got = append(got, l[0].Class)
	}
	if !reflect.DeepEqual(got, []string{"lp", "ln", "up", "un"}) {
		t.Errorf("the lists reached the host as %v", got)
	}
	if g := f.derived[3][0]; g.Owner != "o" || g.Recipient != "r" || g.Verb != "v" || g.ResourceId != "id" {
		t.Errorf("the grant reached the host as %+v", g)
	}
	if f.lifespan.IsNone() || f.lifespan.Some().Iso8601 != "PT1H30M" {
		t.Errorf("the lifespan reached the host as %+v", f.lifespan)
	}

	if _, err := DeriveFromWallet(Bounds{}, None[Lifespan]()); err != nil || f.lifespan.IsSome() {
		t.Errorf("an indefinite wallet derivation sent %+v, %v", f.lifespan, err)
	}

	f.fail(perm.MakePermissionErrorNotPermitted("card:derive is not permitted"))
	_, err = DeriveFromWallet(Bounds{}, None[Lifespan]())
	var pe *PermissionError
	if !errors.As(err, &pe) || pe.Kind != PermissionNotPermitted || pe.Message != "card:derive is not permitted" {
		t.Fatalf("a refusal became %v", err)
	}
	func() {
		defer func() {
			if r := recover(); !errors.As(asError(r), &pe) {
				t.Errorf("MustDerive on a refusal gave %v", r)
			}
		}()
		parent.MustDerive(Bounds{}, None[Lifespan]())
	}()
}

func TestRevokeAndInstallMoveTheCardEvenWhenRefused(t *testing.T) {
	f := newFakeCards(t)
	card := f.card(1, 1)
	copied := card
	if n, err := card.Revoke(); err != nil || n != 3 {
		t.Fatalf("revoke gave %d, %v", n, err)
	}
	mustPanicMoved(t, "ID", func() { copied.ID() })
	mustPanicMoved(t, "Revoke", func() { _, _ = card.Revoke() })

	f.fail(perm.MakePermissionErrorInstallRecipientMismatch("no recipient matches"))
	card = f.card(1, 2)
	h := card.st.h
	target := AgentID{ComponentID: UUID{15: 8}, AgentID: "target(\"x\")"}
	err := card.Install(AgentHolder(target))
	var pe *PermissionError
	if !errors.As(err, &pe) || pe.Kind != PermissionInstallRecipientMismatch {
		t.Fatalf("a mismatched install gave %v", err)
	}
	if f.consumed[len(f.consumed)-1] != h {
		t.Error("the host did not receive the card's handle")
	}
	if f.target.Tag() != perm.HolderAgent || f.target.Agent().AgentId != target.AgentID || f.target.Agent().ComponentId.Uuid.LowBits != 8 {
		t.Errorf("the target reached the host as %+v", f.target)
	}
	mustPanicMoved(t, "Inspect", func() { _, _ = card.Inspect() })
	mustPanicMoved(t, "Install", func() { _ = card.Install(AgentHolder(target)) })

	// The card is moved before the host sees it: a host that fails without
	// returning has still taken the handle.
	trapping := f.card(2, 2)
	revoke := f.ops().revoke
	f.ops().revoke = func(c *types.PermissionCard) witTypes.Result[uint32, perm.PermissionError] {
		if trapping.st.h != nil {
			t.Error("the card was still held during the revoke")
		}
		panic("trap")
	}
	func() {
		defer func() { _ = recover() }()
		_, _ = trapping.Revoke()
	}()
	mustPanicMoved(t, "ID after a trapped revoke", func() { trapping.ID() })
	f.ops().revoke = revoke

	f.err = witTypes.None[perm.PermissionError]()
	open := PolymorphicPermissionCard{st: &permissionCardState{h: &types.PermissionCard{}}}
	if _, err := open.Revoke(); err != nil {
		t.Fatal(err)
	}
	mustPanicMoved(t, "Derive", func() { _, _ = open.Derive(Bounds{}, None[Lifespan]()) })
}

func TestAZeroHolderIsRefusedBeforeTheCardMoves(t *testing.T) {
	f := newFakeCards(t)
	card := f.card(1, 1)
	func() {
		defer func() {
			if recover() == nil {
				t.Error("installing on a zero Holder succeeded")
			}
		}()
		_ = card.Install(Holder{})
	}()
	if card.st.h == nil || len(f.consumed) != 0 {
		t.Error("a refused install moved the card")
	}
	if AccountHolder(UUID{15: 1}).wit.Tag() != perm.HolderAccount || AppHolder(UUID{15: 1}).wit.Tag() != perm.HolderApp {
		t.Error("the holder constructors build the wrong cases")
	}
}

func TestWalletHandlesAreReleasedOnce(t *testing.T) {
	f := newFakeCards(t)
	cards := Wallet()
	if len(cards) != 2 {
		t.Fatalf("wallet gave %d cards", len(cards))
	}
	kept := cards[0]
	ReleaseCards(cards[1:]...)
	cards[1].Release()
	if len(f.released) != 1 || f.released[0] == kept.st.h {
		t.Fatalf("released %v", f.released)
	}
	mustPanicMoved(t, "ID", func() { cards[1].ID() })
	if kept.st.h == nil {
		t.Error("releasing one card moved another")
	}
	if v := WalletVersion(); v.Generation != 7 || !reflect.DeepEqual(v.WalletIDHash, []byte{1, 2}) {
		t.Errorf("wallet version %+v", v)
	}
}

func TestDeriveScopeBorrowsItsParents(t *testing.T) {
	f := newFakeCards(t)
	a, b := f.card(1, 1), f.card(1, 2)
	poly := PolymorphicPermissionCard{st: &permissionCardState{h: &types.PermissionCard{}}}
	f.next = &types.PermissionCard{}
	scope, err := DeriveScope(Bounds{LowerPositive: []PatternGrant{{Class: "card"}}}, a, poly, b)
	if err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(f.scope, []*types.PermissionCard{a.st.h, poly.st.h, b.st.h}) {
		t.Error("the parents did not reach the host in order")
	}
	if a.st.h == nil || b.st.h == nil || poly.st.h == nil {
		t.Error("deriving a scope card moved a parent")
	}
	if got := scopeCardHandle(scope); got.IsNone() || got.Some() != f.next || scope.st.h == nil {
		t.Error("attaching a scope card did not lend its handle")
	}
	if scopeCardHandle(nil).IsSome() {
		t.Error("no scope card became one")
	}
	_, _ = scope.Revoke()
	mustPanicMoved(t, "attaching", func() { scopeCardHandle(scope) })
	mustPanicMoved(t, "deriving from", func() { _, _ = DeriveScope(Bounds{}, a, scope) })
}

func TestClientScopeCardIsADefaultOnACopy(t *testing.T) {
	f := newFakeCards(t)
	card := f.card(1, 1)
	plain := Client[scopeTestID]{agentType: "Profile"}
	scoped := plain.WithScopeCard(card)
	if plain.scopeCard != nil || scoped.scopeCard == nil {
		t.Error("WithScopeCard changed the original client or did not set the copy")
	}
	m := MethodDef[scopeTestID, Unit, Unit]{name: "m"}.WithScopeCard(card)
	if m.card == nil {
		t.Error("the method did not keep its scope card")
	}
}

func TestCardAccessorsConvertTheHostValues(t *testing.T) {
	f := newFakeCards(t)
	parent, child := f.card(1, 2), f.card(3, 4)
	f.parents[child.st.h] = []types.CardId{f.ids[parent.st.h]}
	if parent.ID() == child.ID() || child.Parents()[0] != parent.ID() {
		t.Error("card ids do not compare by identity")
	}
	seen := map[CardID]bool{parent.ID(): true}
	if !seen[child.Parents()[0]] {
		t.Error("a card id is not usable as a map key")
	}
	at, ok := child.ExpiresAt().Get()
	if !ok || !at.Equal(time.Unix(1_700_000_000, 5)) {
		t.Errorf("expires at %v", at)
	}
	v := child.MustInspect()
	if len(v.LowerPositive) != 1 || v.LowerPositive[0].ResourceID != "a" || len(v.UpperNegative) != 1 || v.UpperNegative[0].Class != "blob" || len(v.LowerNegative) != 0 {
		t.Errorf("view %+v", v)
	}
	if ListKernelModules()[0].ClassName != "kv" || ValidateGrant(PatternGrant{}) != nil {
		t.Error("kernel introspection")
	}
}

// Nothing turns data into a card, and a card turns into no data.
func TestCardsCannotBeForgedOrSerialized(t *testing.T) {
	f := newFakeCards(t)
	card := f.card(0x1111222233334444, 0x5555666677778888)
	id := card.ID()
	for _, s := range []string{fmt.Sprint(id), fmt.Sprintf("%+v", id), fmt.Sprintf("%#v", id), fmt.Sprintf("%v", []CardID{id})} {
		if strings.Contains(s, "1111") || strings.Contains(s, "8888") || strings.Contains(strings.ToLower(s), "1229782938247303441") {
			t.Errorf("a card id renders its value: %s", s)
		}
	}
	for _, v := range []any{card, PolymorphicPermissionCard(card), struct{ C PermissionCard }{card}} {
		if out, err := json.Marshal(v); err == nil {
			t.Errorf("a card serialized as %s", out)
		}
	}
	var into struct{ C PermissionCard }
	if err := json.Unmarshal([]byte(`{"C":{}}`), &into); err == nil || into.C.st != nil {
		t.Error("JSON built a card")
	}
	var poly PolymorphicPermissionCard
	if err := json.Unmarshal([]byte(`"card"`), &poly); err == nil || poly.st != nil {
		t.Error("JSON built a polymorphic card")
	}
	for _, typ := range []reflect.Type{reflect.TypeFor[PermissionCard](), reflect.TypeFor[PolymorphicPermissionCard](), reflect.TypeFor[CardID](), reflect.TypeFor[Holder](), reflect.TypeFor[Lifespan]()} {
		for i := range typ.NumField() {
			if typ.Field(i).IsExported() {
				t.Errorf("%s exports field %s", typ, typ.Field(i).Name)
			}
		}
	}
	// The zero value carries no handle.
	mustPanicMoved(t, "a zero card's ID", func() { PermissionCard{}.ID() })
	mustPanicMoved(t, "a zero card's Inspect", func() { _, _ = PolymorphicPermissionCard{}.Inspect() })
	PermissionCard{}.Release()
}

func TestLifespans(t *testing.T) {
	for d, want := range map[time.Duration]string{
		90 * time.Minute:                           "PT1H30M",
		36*time.Hour + 1500*time.Millisecond:       "P1DT12H1.5S",
		48 * time.Hour:                             "P2D",
		time.Nanosecond:                            "PT0.000000001S",
		24*time.Hour + 2*time.Minute + time.Second: "P1DT2M1S",
	} {
		if got := LifespanOf(d).String(); got != want {
			t.Errorf("LifespanOf(%s) = %s, want %s", d, got, want)
		}
	}
	if l, err := ParseLifespan("P5M"); err != nil || l.String() != "P5M" {
		t.Errorf("a calendar duration gave %v, %v", l, err)
	}
	for _, bad := range []string{"", "P", "30D", "pT1H"} {
		if _, err := ParseLifespan(bad); err == nil {
			t.Errorf("ParseLifespan(%q) succeeded", bad)
		}
	}
	for name, f := range map[string]func(){
		"a zero duration":  func() { LifespanOf(0) },
		"a negative one":   func() { LifespanOf(-time.Second) },
		"a zero Lifespan":  func() { lifespanToWit(Some(Lifespan{})) },
		"a malformed text": func() { MustParseLifespan("1h") },
	} {
		func() {
			defer func() {
				if recover() == nil {
					t.Errorf("%s was accepted", name)
				}
			}()
			f()
		}()
	}
}

func TestOutsideAComponentCardOperationsPanic(t *testing.T) {
	defer func() {
		if r := recover(); !errors.Is(asError(r), ErrPermissionsOutsideComponent) {
			t.Errorf("Wallet off the wasm target gave %v", r)
		}
	}()
	Wallet()
}

type scopeTestID struct{ Name string }
