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

	"github.com/golemcloud/golem/sdks/go/golem/internal/engine"
	guestExports "github.com/golemcloud/golem/sdks/go/golem/internal/exports/export_golem_agent_guest"
	common "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_common"
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
	wallet   []perm.WalletCard
}

func newFakeCards(t *testing.T) *fakeCards {
	f := &fakeCards{
		ids:     map[*types.PermissionCard]types.CardId{},
		parents: map[*types.PermissionCard][]types.CardId{},
		err:     witTypes.None[perm.PermissionError](),
		wallet: []perm.WalletCard{
			perm.MakeWalletCardConcrete(&types.PermissionCard{}),
			perm.MakeWalletCardTemplate(&types.PermissionCard{}),
			perm.MakeWalletCardScope(&types.PermissionCard{}),
		},
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
		wallet: func() []perm.WalletCard { return f.wallet },
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
	open := PolymorphicPermissionCard{pst: &permissionCardState{h: &types.PermissionCard{}}}
	if _, err := open.Revoke(); err != nil {
		t.Fatal(err)
	}
	mustPanicMoved(t, "Derive", func() { _, _ = open.Derive(Bounds{}, None[Lifespan]()) })

	f.fail(perm.MakePermissionErrorInstallRecipientMismatch("no recipient matches"))
	open = PolymorphicPermissionCard{pst: &permissionCardState{h: &types.PermissionCard{}}}
	h = open.pst.h
	if err := open.Install(AgentHolder(target)); !errors.As(err, &pe) || pe.Kind != PermissionInstallRecipientMismatch || f.consumed[len(f.consumed)-1] != h {
		t.Fatalf("a mismatched install of a polymorphic card gave %v", err)
	}
	mustPanicMoved(t, "Inspect after a refused install", func() { _, _ = open.Inspect() })
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

// Each wallet card comes back as the type of its kind and as no other, and
// shares its handle with the WalletCard.
func TestWalletLabelsEachCardWithItsKind(t *testing.T) {
	f := newFakeCards(t)
	cards := Wallet()
	if len(cards) != 3 {
		t.Fatalf("wallet gave %d cards", len(cards))
	}
	concrete, template, scope := cards[0], cards[1], cards[2]
	if concrete.Kind() != WalletCardConcrete || template.Kind() != WalletCardTemplate || scope.Kind() != WalletCardScope {
		t.Fatalf("kinds %v %v %v", concrete.Kind(), template.Kind(), scope.Kind())
	}
	if c, ok := concrete.Concrete(); !ok || c.st.h != f.wallet[0].Concrete() {
		t.Error("a concrete card is not a PermissionCard of its handle")
	}
	if c, ok := template.Template(); !ok || c.pst.h != f.wallet[1].Template() {
		t.Error("a template card is not a PolymorphicPermissionCard of its handle")
	}
	if c, ok := scope.Scope(); !ok || c.sst.h != f.wallet[2].Scope() {
		t.Error("the lent scope card is not a ScopeCard of its handle")
	}
	for name, ok := range map[string]bool{
		"concrete as template": second(concrete.Template()), "concrete as scope": second(concrete.Scope()),
		"template as concrete": second(template.Concrete()), "template as scope": second(template.Scope()),
		"scope as concrete": second(scope.Concrete()), "scope as template": second(scope.Template()),
		"zero as concrete": second(WalletCard{}.Concrete()), "zero as template": second(WalletCard{}.Template()),
		"zero as scope": second(WalletCard{}.Scope()),
	} {
		if ok {
			t.Errorf("the wallet gave %s", name)
		}
	}
	if s := []string{concrete.Kind().String(), template.Kind().String(), scope.Kind().String()}; !reflect.DeepEqual(s, []string{"concrete", "template", "scope"}) {
		t.Errorf("kind names %v", s)
	}

	if at, ok := scope.ExpiresAt().Get(); !ok || !at.Equal(time.Unix(1_700_000_000, 5)) || scope.MustInspect().LowerPositive[0].ResourceID != "a" {
		t.Error("a wallet card does not read its card")
	}
	mustPanicMoved(t, "ID of the zero WalletCard", func() { WalletCard{}.ID() })
	poly, _ := template.Template()
	poly.Release()
	mustPanicMoved(t, "ID after the accessor's card was released", func() { template.ID() })
	ReleaseCards(cards...)
	if !reflect.DeepEqual(f.released, []*types.PermissionCard{f.wallet[1].Template(), f.wallet[0].Concrete(), f.wallet[2].Scope()}) {
		t.Errorf("released %v", f.released)
	}
	if c, _ := concrete.Concrete(); c.st.h != nil {
		t.Error("releasing the WalletCard left its card usable")
	}
	WalletCard{}.Release()
	if v := WalletVersion(); v.Generation != 7 || !reflect.DeepEqual(v.WalletIDHash, []byte{1, 2}) {
		t.Errorf("wallet version %+v", v)
	}
}

func second[A any](_ A, ok bool) bool { return ok }

func TestDeriveScopeBorrowsItsParents(t *testing.T) {
	f := newFakeCards(t)
	a, b := f.card(1, 1), f.card(1, 2)
	poly := PolymorphicPermissionCard{pst: &permissionCardState{h: &types.PermissionCard{}}}
	f.next = &types.PermissionCard{}
	scope, err := DeriveScope(Bounds{LowerPositive: []PatternGrant{{Class: "card"}}}, a, poly, b)
	if err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(f.scope, []*types.PermissionCard{a.st.h, poly.pst.h, b.st.h}) {
		t.Error("the parents did not reach the host in order")
	}
	if a.st.h == nil || b.st.h == nil || poly.pst.h == nil {
		t.Error("deriving a scope card moved a parent")
	}
	if got := scopeCardHandle(scope); got.IsNone() || got.Some() != f.next || scope.sst.h == nil {
		t.Error("attaching a scope card did not lend its handle")
	}
	if scopeCardHandle(ScopeCard{}).IsSome() {
		t.Error("no scope card became one")
	}

	// A scope card is a parent like any other.
	inner := f.next
	f.next = &types.PermissionCard{}
	if _, err := DeriveScope(Bounds{}, scope, a); err != nil || !reflect.DeepEqual(f.scope, []*types.PermissionCard{inner, a.st.h}) {
		t.Errorf("a scope card as a parent reached the host as %v, %v", f.scope, err)
	}

	// Wallet cards of every kind, the lent scope card among them, are parents
	// too, and keep their handles.
	wallet := Wallet()
	f.next = &types.PermissionCard{}
	if _, err := DeriveScope(Bounds{}, wallet[0], wallet[1], wallet[2]); err != nil || !reflect.DeepEqual(f.scope, []*types.PermissionCard{f.wallet[0].Concrete(), f.wallet[1].Template(), f.wallet[2].Scope()}) {
		t.Errorf("wallet cards as parents reached the host as %v, %v", f.scope, err)
	}
	concrete, _ := wallet[0].Concrete()
	template, _ := wallet[1].Template()
	lent, _ := wallet[2].Scope()
	if concrete.st.h != f.wallet[0].Concrete() || template.pst.h != f.wallet[1].Template() || lent.sst.h != f.wallet[2].Scope() {
		t.Error("deriving a scope card moved a wallet card")
	}
	if got := scopeCardHandle(lent); got.IsNone() || got.Some() != f.wallet[2].Scope() {
		t.Error("the lent scope card cannot be attached after it was a parent")
	}

	scope.Release()
	mustPanicMoved(t, "attaching", func() { scopeCardHandle(scope) })
	mustPanicMoved(t, "deriving from", func() { _, _ = DeriveScope(Bounds{}, a, scope) })
	mustPanicMoved(t, "inspecting", func() { _, _ = scope.Inspect() })

	f.fail(perm.MakePermissionErrorNotPermitted("no card:derive"))
	if _, err := DeriveScope(Bounds{}, a); !isKind(err, PermissionNotPermitted) {
		t.Errorf("a refused scope derivation gave %v", err)
	}
}

func isKind(err error, kind PermissionErrorKind) bool {
	var pe *PermissionError
	return errors.As(err, &pe) && pe.Kind == kind
}

type scopeCallIn struct {
	First  PermissionCard
	Second PermissionCard
}

// An awaited call carries the scope card attached to it, else the client's
// default, else none; and it resolves the scope card before it encodes the
// input, so a released scope card leaves every card in the input with the
// caller.
func TestAnAwaitedCallResolvesItsScopeCardBeforeItsInput(t *testing.T) {
	f := newFakeCards(t)
	perCall := ScopeCard{sst: &permissionCardState{h: &types.PermissionCard{}}}
	byDefault := ScopeCard{sst: &permissionCardState{h: &types.PermissionCard{}}}
	m := MethodDef[scopeTestID, Unit, Unit]{name: "m"}
	plain := Client[scopeTestID]{agentType: "Profile"}
	scoped := plain.WithScopeCard(byDefault)
	if plain.scopeCard.sst != nil {
		t.Error("WithScopeCard changed the original client")
	}

	for name, tc := range map[string]struct {
		c     Client[scopeTestID]
		scope ScopeCard
		want  *types.PermissionCard
	}{
		"no card":                   {plain, ScopeCard{}, nil},
		"the client's default":      {scoped, ScopeCard{}, byDefault.sst.h},
		"the call's own card":       {plain, perCall, perCall.sst.h},
		"the call's card over both": {scoped, m.WithScopeCard(perCall).card, perCall.sst.h},
	} {
		_, got, err := m.awaitedCallArgs(tc.c, Unit{}, tc.scope)
		if err != nil || got.IsSome() != (tc.want != nil) || got.IsSome() && got.Some() != tc.want {
			t.Errorf("%s: the call carried %v, %v", name, got, err)
		}
	}

	arg := f.card(5, 5)
	byDefault.Release()
	inMethod := MethodDef[scopeTestID, scopeCallIn, Unit]{name: "m"}
	mustPanicMoved(t, "a call with a released scope card", func() {
		_, _, _ = inMethod.awaitedCallArgs(scoped, scopeCallIn{First: arg, Second: f.card(6, 6)}, ScopeCard{})
	})
	if arg.st.h == nil {
		t.Error("a call refused for its scope card moved an input card")
	}

	for name, attach := range map[string]func(){
		"Client.WithScopeCard":    func() { plain.WithScopeCard(ScopeCard{}) },
		"MethodDef.WithScopeCard": func() { m.WithScopeCard(ScopeCard{}) },
	} {
		func() {
			defer func() {
				if r := recover(); r == nil || !strings.Contains(fmt.Sprint(r), "a zero ScopeCard") {
					t.Errorf("%s accepted the zero ScopeCard: %v", name, r)
				}
			}()
			attach()
		}()
	}
}

// An encode that fails part way gives the cards it already moved back to
// their owners, as nothing reached the host.
func TestAFailedEncodeGivesTheMovedCardsBack(t *testing.T) {
	f := newFakeCards(t)
	first, second := f.card(1, 1), f.card(2, 2)
	second.Release()
	m := MethodDef[scopeTestID, scopeCallIn, Unit]{name: "m"}
	if _, err := m.encodeInput(scopeCallIn{First: first, Second: second}); err == nil || !strings.Contains(err.Error(), ErrPermissionCardMoved.Error()) {
		t.Fatalf("encoding a moved card gave %v", err)
	}
	if first.st.h == nil {
		t.Error("the card encoded before the failure stayed moved")
	}
	func() {
		defer func() { _ = recover() }()
		EncodeTypedValue(scopeCallIn{First: first, Second: second})
	}()
	if first.st.h == nil {
		t.Error("a failed EncodeTypedValue kept a moved card")
	}
	open := PolymorphicPermissionCard{pst: &permissionCardState{h: &types.PermissionCard{}}}
	func() {
		defer func() { _ = recover() }()
		EncodeTypedValue(struct {
			Open   PolymorphicPermissionCard
			Second PermissionCard
		}{open, second})
	}()
	if open.pst.h == nil {
		t.Error("a failed EncodeTypedValue kept a moved polymorphic card")
	}
}

type cardThenText struct {
	Card PermissionCard
	Text string
}

type cardThenNumber struct {
	Card   PermissionCard
	Number uint32
}

// Decoding a card takes it out of the value: a second decode fails rather
// than give a second card the same handle. A decode that fails leaves the
// value as it was.
func TestDecodingACardTakesItOutOfTheValue(t *testing.T) {
	f := newFakeCards(t)
	card := f.card(1, 1)
	h := card.st.h
	value := EncodeTypedValue(cardThenText{Card: card, Text: "t"})
	copied := value

	if _, err := DecodeTypedValue[cardThenNumber](value); err == nil {
		t.Fatal("a mismatched decode succeeded")
	}
	got, err := DecodeTypedValue[cardThenText](value)
	if err != nil || got.Card.st.h != h {
		t.Fatalf("decoding after a failed decode gave %v, %v", got, err)
	}
	if again, err := DecodeTypedValue[cardThenText](copied); !errors.Is(err, ErrPermissionCardTaken) || again.Card.st != nil {
		t.Errorf("a second decode gave %v, %v", again.Card, err)
	}
	if _, err := DecodeTypedValue[PolymorphicPermissionCard](EncodeTypedValue(PolymorphicPermissionCard{pst: &permissionCardState{h: h}})); err != nil {
		t.Errorf("a polymorphic card did not decode: %v", err)
	}
}

// A scope card, a card id, a holder, a lifespan and a wallet card cannot be
// sent: a type that holds one is a definition error, and encoding one through
// a type nobody validated fails.
func TestCardManagementValuesHaveNoSchema(t *testing.T) {
	type holds struct {
		Scope    ScopeCard
		ID       CardID
		Holder   Holder
		Lifespan Lifespan
		Wallet   WalletCard
	}
	g := engine.GraphBuilder{E: defs.Engine}
	g.Node(defs.Compile(reflect.TypeFor[holds]()))
	for typ, name := range map[reflect.Type]string{
		reflect.TypeFor[ScopeCard](): "golem.ScopeCard", reflect.TypeFor[CardID](): "golem.CardID",
		reflect.TypeFor[Holder](): "golem.Holder", reflect.TypeFor[Lifespan](): "golem.Lifespan",
		reflect.TypeFor[WalletCard](): "golem.WalletCard",
	} {
		if why := g.Invalids[typ]; !strings.HasPrefix(why, name+" cannot be sent") {
			t.Errorf("a field of type %s compiled: %q", name, why)
		}
	}
	for name, v := range map[string]any{
		"CardID":     struct{ ID CardID }{},
		"Holder":     struct{ H Holder }{AgentHolder(AgentID{AgentID: "a"})},
		"Lifespan":   struct{ L Lifespan }{LifespanOf(time.Hour)},
		"WalletCard": struct{ W WalletCard }{WalletCard{kind: WalletCardConcrete, wst: &permissionCardState{h: &types.PermissionCard{}}}},
	} {
		func() {
			defer func() {
				if r := recover(); !strings.Contains(fmt.Sprint(r), "cannot be sent") {
					t.Errorf("encoding a %s gave %v", name, r)
				}
			}()
			engine.EncodeWith(defs.Compile(reflect.TypeOf(v)), reflect.ValueOf(v))
		}()
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

// A card id renders as a placeholder under every verb, inside other values,
// and as JSON, and never shows the identifier.
func TestCardIDsNeverRenderTheIdentifier(t *testing.T) {
	f := newFakeCards(t)
	const hi, lo = 0x1111222233334444, 0x5555666677778888
	id := f.card(hi, lo).ID()
	// Every rendering of either half that a verb could produce.
	var leaks []string
	for _, n := range []uint64{hi, lo} {
		for _, verb := range []string{"%d", "%x", "%X", "%o", "%b"} {
			leaks = append(leaks, fmt.Sprintf(verb, n))
		}
		leaks = append(leaks, fmt.Sprintf("%x", n)[:8], fmt.Sprintf("%d", n)[:8])
	}
	type exported struct{ ID CardID }
	type unexported struct{ id CardID }
	var rendered []string
	for _, verb := range []string{"%v", "%+v", "%#v", "%s", "%q", "%d", "%x", "%X", "%o", "%O", "%b", "%c", "%U", "%e", "%f", "%g", "%t", "%p", "%T", "%10.3v", "%-8d", "%#x"} {
		for _, v := range []any{id, &id, []CardID{id}, map[CardID]int{id: 1}, exported{id}, unexported{id}, &unexported{id}} {
			rendered = append(rendered, fmt.Sprintf(verb, v))
		}
	}
	for _, v := range []any{id, exported{id}, []CardID{id}, map[CardID]int{id: 1}} {
		out, err := json.Marshal(v)
		if err != nil {
			t.Errorf("%T did not serialize: %v", v, err)
		}
		rendered = append(rendered, string(out))
	}
	if out, _ := json.Marshal(id); string(out) != `"golem.CardID{…}"` {
		t.Errorf("a card id serialized as %s", out)
	}
	for _, r := range rendered {
		for _, leak := range leaks {
			if strings.Contains(strings.ToLower(r), strings.ToLower(leak)) {
				t.Errorf("%q shows the identifier as %s", r, leak)
			}
		}
	}
}

// Nothing turns data into a card, and a card turns into no data.
func TestCardsCannotBeForgedOrSerialized(t *testing.T) {
	f := newFakeCards(t)
	card := f.card(1, 2)
	poly := PolymorphicPermissionCard{pst: &permissionCardState{h: &types.PermissionCard{}}}
	scope := ScopeCard{sst: &permissionCardState{h: &types.PermissionCard{}}}
	for _, v := range []any{card, poly, scope, struct{ C PermissionCard }{card}, Wallet()[0]} {
		if out, err := json.Marshal(v); err == nil {
			t.Errorf("a card serialized as %s", out)
		}
	}
	var into struct{ C PermissionCard }
	if err := json.Unmarshal([]byte(`{"C":{}}`), &into); err == nil || into.C.st != nil {
		t.Error("JSON built a card")
	}
	var intoPoly PolymorphicPermissionCard
	if err := json.Unmarshal([]byte(`"card"`), &intoPoly); err == nil || intoPoly.pst != nil {
		t.Error("JSON built a polymorphic card")
	}
	var intoScope ScopeCard
	if err := json.Unmarshal([]byte(`{}`), &intoScope); err == nil || intoScope.sst != nil {
		t.Error("JSON built a scope card")
	}
	var intoWallet WalletCard
	if err := json.Unmarshal([]byte(`{}`), &intoWallet); err == nil || intoWallet.wst != nil || intoWallet.kind != 0 {
		t.Error("JSON built a wallet card")
	}
	for _, typ := range []reflect.Type{reflect.TypeFor[PermissionCard](), reflect.TypeFor[PolymorphicPermissionCard](), reflect.TypeFor[ScopeCard](), reflect.TypeFor[CardID](), reflect.TypeFor[Holder](), reflect.TypeFor[Lifespan](), reflect.TypeFor[WalletCard]()} {
		for i := range typ.NumField() {
			if typ.Field(i).IsExported() {
				t.Errorf("%s exports field %s", typ, typ.Field(i).Name)
			}
		}
	}
	// The zero value carries no handle.
	mustPanicMoved(t, "a zero card's ID", func() { PermissionCard{}.ID() })
	mustPanicMoved(t, "a zero card's Inspect", func() { _, _ = PolymorphicPermissionCard{}.Inspect() })
	mustPanicMoved(t, "a zero scope card's ID", func() { ScopeCard{}.ID() })
	PermissionCard{}.Release()
	ScopeCard{}.Release()
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
	// Every LifespanOf rendering parses, as do the other forms the host reads.
	for _, good := range []string{"PT1H30M", "P1DT12H1.5S", "P2D", "PT0.000000001S", "P1DT2M1S", "PT30M", "PT0H1S", "P0DT1H", "PT10.50S",
		"PT9223372036.854775807S", "P106751DT23H47M16.854775807S"} {
		if l, err := ParseLifespan(good); err != nil || l.String() != good {
			t.Errorf("ParseLifespan(%q) gave %v, %v", good, l, err)
		}
	}
	// Calendar units, malformed text, fractions past nanoseconds and totals
	// past an int64 of nanoseconds fail, as on the host. A zero duration fails
	// too, which the host accepts: LifespanOf refuses it as well. A leading
	// '.' in the seconds fails, which the host may read.
	for _, bad := range []string{
		"", "P", "PT", "30D", "pT1H", "-PT1S", "P5M", "P1M", "P1Y", "P1W", "PT0S", "P0D", "PT0.0S",
		"PT1M1H", "P1D2D", "PT1H1H", "P1.5D", "PT1.5M", "PT1.S", "PT.5S", "PT1.2.3S", "P1DT", "PT1", "P1H", "PTS", "P1DT1D",
		"PT1.0000000001S", "PT9223372036.854775808S", "P106751DT23H47M16.854775808S", "P106752D", "PT99999999999999999999S",
	} {
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

type embedsCard struct {
	PermissionCard
	Note string
}

// A struct that embeds a card has the card's methods but is a record of its
// own fields, the card among them.
func TestAStructEmbeddingACardIsARecord(t *testing.T) {
	f := newFakeCards(t)
	card := f.card(1, 1)
	h := card.st.h
	got, err := DecodeTypedValue[embedsCard](EncodeTypedValue(embedsCard{PermissionCard: card, Note: "n"}))
	if err != nil || got.Note != "n" || got.PermissionCard.st.h != h {
		t.Fatalf("an embedding struct round-tripped as %+v, %v", got, err)
	}
	g := engine.GraphBuilder{E: defs.Engine}
	root := g.Node(defs.Compile(reflect.TypeFor[embedsCard]()))
	if body := g.Build().TypeNodes[root].Body; body.Tag() != types.SchemaTypeBodyRecordType {
		t.Errorf("an embedding struct is typed as tag %d", body.Tag())
	}
}

type cardThenScope struct {
	Card  PermissionCard
	Scope ScopeCard
}

// A scope card reached through a type nobody validated still fails to encode,
// and the encode gives back the cards it moved.
func TestEncodingAScopeCardFailsAndGivesTheCardsBack(t *testing.T) {
	f := newFakeCards(t)
	card := f.card(1, 1)
	func() {
		defer func() {
			if r := recover(); !strings.Contains(fmt.Sprint(r), "ScopeCard cannot be sent") {
				t.Errorf("encoding a scope card gave %v", r)
			}
		}()
		EncodeTypedValue(cardThenScope{Card: card, Scope: ScopeCard{sst: &permissionCardState{h: &types.PermissionCard{}}}})
	}()
	if card.st.h == nil {
		t.Error("the failed encode kept the card")
	}
}

// A decode of a received value that fails gives every card in the value back
// to the host: the ones it adopted and the ones it did not reach.
func TestAFailedDecodeOfAReceivedValueReleasesItsCards(t *testing.T) {
	f := newFakeCards(t)
	first, second := f.card(1, 1), PolymorphicPermissionCard{pst: &permissionCardState{h: &types.PermissionCard{}}}
	h1, h2 := first.st.h, second.pst.h
	tree := witTypes.Some(EncodeTypedValue(struct {
		First  PermissionCard
		Text   string
		Second PolymorphicPermissionCard
	}{first, "t", second}).wit.Value)
	type wrong struct {
		First  PermissionCard
		Number uint32
		Second PolymorphicPermissionCard
	}
	if _, err := decodeOutput[wrong]("target", "m", tree); err == nil {
		t.Fatal("a mismatched result decoded")
	}
	if len(f.released) != 2 || f.released[0] != h1 && f.released[1] != h1 || f.released[0] != h2 && f.released[1] != h2 {
		t.Errorf("released %v, want both cards", f.released)
	}
}

// A received parameter list gives back the cards a decode does not keep:
// all of them when the list is malformed, and those in values past the
// parameters when it decodes.
func TestDecodingParametersReleasesTheCardsItDoesNotKeep(t *testing.T) {
	type oneParam struct{ Card PermissionCard }
	fields := defs.StructFields(reflect.TypeFor[oneParam]())
	for name, tc := range map[string]struct {
		nodes   func(first, extra *types.PermissionCard) []types.SchemaValueNode
		root    int32
		decodes bool
		want    int
	}{
		"a root that is not a record": {func(first, extra *types.PermissionCard) []types.SchemaValueNode {
			return []types.SchemaValueNode{types.MakeSchemaValueNodePermissionCardHandle(first)}
		}, 0, false, 1},
		"a record with too few values": {func(first, extra *types.PermissionCard) []types.SchemaValueNode {
			return []types.SchemaValueNode{types.MakeSchemaValueNodePermissionCardHandle(first), types.MakeSchemaValueNodeRecordValue(nil)}
		}, 1, false, 1},
		"a value past the parameters": {func(first, extra *types.PermissionCard) []types.SchemaValueNode {
			return []types.SchemaValueNode{
				types.MakeSchemaValueNodePermissionCardHandle(first), types.MakeSchemaValueNodePermissionCardHandle(extra),
				types.MakeSchemaValueNodeRecordValue([]int32{0, 1}),
			}
		}, 2, true, 1},
	} {
		f := newFakeCards(t)
		first, extra := &types.PermissionCard{}, &types.PermissionCard{}
		dst := reflect.New(reflect.TypeFor[oneParam]()).Elem()
		err := decodeParams(types.SchemaValueTree{ValueNodes: tc.nodes(first, extra), Root: tc.root}, fields, dst, nil)
		if (err == nil) != tc.decodes || len(f.released) != tc.want {
			t.Errorf("%s: %v, released %v", name, err, f.released)
			continue
		}
		if tc.decodes && (f.released[0] != extra || dst.Interface().(oneParam).Card.st.h != first) {
			t.Errorf("%s: released %v and kept %v", name, f.released, dst.Interface())
		}
	}
}

// An agent invocation refused before its decode, and a result read as Unit,
// give their cards back to the host.
func TestRefusedInvocationsAndUnitResultsReleaseTheirCards(t *testing.T) {
	f := newFakeCards(t)
	refused := &types.PermissionCard{}
	prev := active
	active = nil
	t.Cleanup(func() { active = prev })
	input := types.SchemaValueTree{ValueNodes: []types.SchemaValueNode{
		types.MakeSchemaValueNodePermissionCardHandle(refused), types.MakeSchemaValueNodeRecordValue([]int32{0}),
	}, Root: 1}
	if res := guestExports.Exports.Invoke("m", input, common.MakePrincipalAnonymous()); res.IsOk() {
		t.Fatal("an uninitialized agent ran a method")
	}

	unit := &types.PermissionCard{}
	if _, err := decodeOutput[Unit]("target", "m", witTypes.Some(types.SchemaValueTree{
		ValueNodes: []types.SchemaValueNode{types.MakeSchemaValueNodePermissionCardHandle(unit)},
	})); err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(f.released, []*types.PermissionCard{refused, unit}) {
		t.Errorf("released %v, want the refused input's and the unit result's cards", f.released)
	}
}
