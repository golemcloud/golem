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
	"errors"
	"fmt"
	"strings"
	"time"

	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	perm "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_permissions_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// Permission-card management.
//
// An agent holds cards in its wallet and may derive narrower cards from them,
// inspect them, install them on other agents and revoke them. What an agent may
// do with cards is itself granted by cards: the card:derive, card:inspect,
// card:install and card:revoke verbs. A call the agent's cards do not permit
// returns a [*PermissionError] of kind [PermissionNotPermitted].
//
//	card, err := golem.DeriveFromWallet(golem.Bounds{
//	    LowerPositive: []golem.PatternGrant{{Class: "kv", Owner: "*", Recipient: "*", Verb: "read", ResourceID: "orders.*"}},
//	}, golem.Some(golem.LifespanOf(time.Hour)))
//	if err != nil { … }
//	err = card.Install(golem.AgentHolder(target))
//
// A card wraps a host handle, and nothing turns data into one: no constructor
// takes a CardID, a string or JSON, and a card does not serialize. Derivation
// inputs are requests the host checks against the parent's grants.
//
// Revoke and Install consume the card, as sending it does: afterwards the card
// is moved, even when the call failed, and every further use panics with
// [ErrPermissionCardMoved]. ID, Parents and ExpiresAt trap when the host no
// longer knows the card.

// ErrPermissionsOutsideComponent reports a card operation outside a component,
// where there is no host to perform it.
var ErrPermissionsOutsideComponent = errors.New("golem: permission cards are only available inside a component")

// AnyPermissionCard is a [PermissionCard] or a [PolymorphicPermissionCard]. No
// other type implements it.
type AnyPermissionCard interface {
	// ID returns the card's identifier.
	ID() CardID
	// Parents returns the identifiers of the cards this card was derived from.
	Parents() []CardID
	// ExpiresAt returns when the card expires, or None for a card without a
	// bounded lifespan.
	ExpiresAt() Option[time.Time]
	// Inspect returns the card's grants. It needs the card:inspect verb.
	Inspect() (CardView, error)
	// Derive derives a narrower child card. It needs the card:derive verb.
	Derive(b Bounds, lifespan Option[Lifespan]) (PermissionCard, error)
	// Revoke revokes the card and every card derived from it, consuming it.
	Revoke() (uint32, error)
	// Install installs the card on target, consuming it.
	Install(target Holder) error
	// Release gives the card's handle back to the host.
	Release()

	cardState() *permissionCardState
	polymorphic() bool
}

var (
	_ AnyPermissionCard = PermissionCard{}
	_ AnyPermissionCard = PolymorphicPermissionCard{}
)

// CardID identifies a card. It is opaque: it can be compared, and used as a
// map key, to tell whether a card's [AnyPermissionCard.Parents] name another
// card, and nothing else.
type CardID struct{ hi, lo uint64 }

// String does not render the identifier, which is private to the host.
func (CardID) String() string { return "golem.CardID{…}" }

// GoString does not render the identifier, which is private to the host.
func (CardID) GoString() string { return "golem.CardID{…}" }

func cardIDFromWit(w types.CardId) CardID {
	return CardID{hi: w.Uuid.HighBits, lo: w.Uuid.LowBits}
}

// PatternGrant is one grant: a verb on resources of a class, owned by owner,
// for recipient. Each field is a pattern in the kernel's grant syntax.
type PatternGrant struct {
	Class      string
	Owner      string
	Recipient  string
	Verb       string
	ResourceID string
}

// CardView is a card's grant lattice: the grants it confers (positive) and the
// grants it withholds (negative), at its lower and upper bound.
type CardView struct {
	LowerPositive []PatternGrant
	LowerNegative []PatternGrant
	UpperPositive []PatternGrant
	UpperNegative []PatternGrant
}

// Bounds is the grant lattice a derivation requests.
//
// For [AnyPermissionCard.Derive] the positive lists name the parent's grants
// the child retains and the negative lists the denials it adds. For
// [DeriveFromWallet] and [DeriveScope] the four lists are the child's complete
// lattice. The host rejects a lattice its parents do not cover.
type Bounds struct {
	LowerPositive []PatternGrant
	LowerNegative []PatternGrant
	UpperPositive []PatternGrant
	UpperNegative []PatternGrant
}

// Lifespan is how long a derived card lives, as an ISO-8601 duration.
type Lifespan struct{ iso string }

// ParseLifespan reads an ISO-8601 duration such as "PT30M" or "P30D". It
// checks only the leading 'P'; the host rejects a duration it cannot read.
func ParseLifespan(iso string) (Lifespan, error) {
	if len(iso) < 2 || iso[0] != 'P' {
		return Lifespan{}, fmt.Errorf("golem: lifespan %q is not an ISO-8601 duration starting with 'P'", iso)
	}
	return Lifespan{iso: iso}, nil
}

// MustParseLifespan is [ParseLifespan] that panics on a malformed duration.
func MustParseLifespan(iso string) Lifespan { return Must(ParseLifespan(iso)) }

// LifespanOf is a lifespan of d, which must be positive.
func LifespanOf(d time.Duration) Lifespan {
	if d <= 0 {
		panic(fmt.Errorf("golem: a card lifespan must be positive, got %s", d))
	}
	return Lifespan{iso: isoDuration(d)}
}

// String returns the ISO-8601 duration.
func (l Lifespan) String() string { return l.iso }

// isoDuration renders a positive duration as P[nD][T[nH][nM][n[.f]S]].
func isoDuration(d time.Duration) string {
	const day = 24 * time.Hour
	var b strings.Builder
	b.WriteByte('P')
	if days := d / day; days > 0 {
		fmt.Fprintf(&b, "%dD", days)
		d -= days * day
	}
	if d == 0 {
		return b.String()
	}
	b.WriteByte('T')
	if h := d / time.Hour; h > 0 {
		fmt.Fprintf(&b, "%dH", h)
		d -= h * time.Hour
	}
	if m := d / time.Minute; m > 0 {
		fmt.Fprintf(&b, "%dM", m)
		d -= m * time.Minute
	}
	if d > 0 {
		s, ns := d/time.Second, d%time.Second
		if ns == 0 {
			fmt.Fprintf(&b, "%dS", s)
		} else {
			fmt.Fprintf(&b, "%d.%sS", s, strings.TrimRight(fmt.Sprintf("%09d", ns), "0"))
		}
	}
	return b.String()
}

// Holder is who a card is installed on: an agent, an account or an
// application. The host currently installs cards on agents only, and rejects
// the other targets with [PermissionNotPermitted].
type Holder struct {
	wit perm.Holder
	ok  bool
}

// AgentHolder is the agent id names.
func AgentHolder(id AgentID) Holder { return Holder{wit: perm.MakeHolderAgent(id.toWit()), ok: true} }

// AccountHolder is the account id names.
func AccountHolder(id UUID) Holder {
	return Holder{wit: perm.MakeHolderAccount(uuidToWit(id)), ok: true}
}

// AppHolder is the application id names.
func AppHolder(id UUID) Holder { return Holder{wit: perm.MakeHolderApp(uuidToWit(id)), ok: true} }

// WalletVersionToken identifies a state of the agent's wallet: the generation
// changes whenever a card is installed in it or removed from it.
type WalletVersionToken struct {
	WalletIDHash []byte
	Generation   uint64
}

// KernelModule is a resource class the permission kernel knows, with its
// verbs.
type KernelModule struct {
	ClassName string
	Verbs     []string
}

// PermissionErrorKind classifies a [PermissionError]. The kinds are the host's
// permission-error cases.
type PermissionErrorKind uint8

const (
	// PermissionNotPermitted means the agent's cards do not permit the call.
	PermissionNotPermitted PermissionErrorKind = iota
	// PermissionLowerBoundTooBroad means the requested lower bound is not
	// covered by the parents.
	PermissionLowerBoundTooBroad
	// PermissionUpperBoundTooBroad means the requested upper bound is not
	// covered by the parents.
	PermissionUpperBoundTooBroad
	// PermissionWalletMultiSourceRequired means no single wallet card covers
	// the request.
	PermissionWalletMultiSourceRequired
	// PermissionCardRevoked means the card was revoked.
	PermissionCardRevoked
	// PermissionCardExpired means the card, or the requested lifespan, outlives
	// what its parent allows.
	PermissionCardExpired
	// PermissionInstallRecipientMismatch means the install target matches none
	// of the card's recipients.
	PermissionInstallRecipientMismatch
	// PermissionUnknownResourceClass means a grant names a class the kernel
	// does not know.
	PermissionUnknownResourceClass
	// PermissionInvalidPattern means a grant, or a lifespan, does not parse.
	PermissionInvalidPattern
	// PermissionInvalidVerb means a grant names a verb its class does not
	// have.
	PermissionInvalidVerb
	// PermissionInvalidOwner means a grant's owner pattern is invalid.
	PermissionInvalidOwner
	// PermissionInvalidRecipient means a grant's recipient pattern is invalid.
	PermissionInvalidRecipient
)

// permissionErrorKindCount pins how many permission-error cases the bindings
// declare. Bump it deliberately, with a new kind, when the WIT gains a case.
const permissionErrorKindCount = 12

var permissionErrorKindNames = [permissionErrorKindCount]string{
	"not-permitted",
	"lower-bound-too-broad",
	"upper-bound-too-broad",
	"wallet-multi-source-required",
	"card-revoked",
	"card-expired",
	"install-recipient-mismatch",
	"unknown-resource-class",
	"invalid-pattern",
	"invalid-verb",
	"invalid-owner",
	"invalid-recipient",
}

// String returns the host's name for the case.
func (k PermissionErrorKind) String() string {
	if int(k) < len(permissionErrorKindNames) {
		return permissionErrorKindNames[k]
	}
	return fmt.Sprintf("PermissionErrorKind(%d)", uint8(k))
}

// PermissionError is a card operation the host refused.
type PermissionError struct {
	Kind    PermissionErrorKind
	Message string
}

func (e *PermissionError) Error() string {
	return "golem: permission card: " + e.Kind.String() + ": " + e.Message
}

func permissionErrorFromWit(w perm.PermissionError) error {
	var msg string
	switch w.Tag() {
	case perm.PermissionErrorNotPermitted:
		msg = w.NotPermitted()
	case perm.PermissionErrorLowerBoundTooBroad:
		msg = w.LowerBoundTooBroad()
	case perm.PermissionErrorUpperBoundTooBroad:
		msg = w.UpperBoundTooBroad()
	case perm.PermissionErrorWalletMultiSourceRequired:
		msg = w.WalletMultiSourceRequired()
	case perm.PermissionErrorCardRevoked:
		msg = w.CardRevoked()
	case perm.PermissionErrorCardExpired:
		msg = w.CardExpired()
	case perm.PermissionErrorInstallRecipientMismatch:
		msg = w.InstallRecipientMismatch()
	case perm.PermissionErrorUnknownResourceClass:
		msg = w.UnknownResourceClass()
	case perm.PermissionErrorInvalidPattern:
		msg = w.InvalidPattern()
	case perm.PermissionErrorInvalidVerb:
		msg = w.InvalidVerb()
	case perm.PermissionErrorInvalidOwner:
		msg = w.InvalidOwner()
	case perm.PermissionErrorInvalidRecipient:
		msg = w.InvalidRecipient()
	default:
		panic(fmt.Sprintf("golem: unhandled permission-error case %d", w.Tag()))
	}
	// The kinds are declared in the WIT case order.
	return &PermissionError{Kind: PermissionErrorKind(w.Tag()), Message: msg}
}

func grantsToWit(gs []PatternGrant) []perm.PatternGrant {
	out := make([]perm.PatternGrant, len(gs))
	for i, g := range gs {
		out[i] = perm.PatternGrant{Class: g.Class, Owner: g.Owner, Recipient: g.Recipient, Verb: g.Verb, ResourceId: g.ResourceID}
	}
	return out
}

func grantsFromWit(gs []perm.PatternGrant) []PatternGrant {
	out := make([]PatternGrant, len(gs))
	for i, g := range gs {
		out[i] = PatternGrant{Class: g.Class, Owner: g.Owner, Recipient: g.Recipient, Verb: g.Verb, ResourceID: g.ResourceId}
	}
	return out
}

func lifespanToWit(l Option[Lifespan]) witTypes.Option[perm.Duration] {
	if v, ok := l.Get(); ok {
		if v.iso == "" {
			panic(errors.New("golem: a zero Lifespan; build one with LifespanOf or ParseLifespan"))
		}
		return witTypes.Some(perm.Duration{Iso8601: v.iso})
	}
	return witTypes.None[perm.Duration]()
}

func cardIDsFromWit(ids []types.CardId) []CardID {
	out := make([]CardID, len(ids))
	for i, id := range ids {
		out[i] = cardIDFromWit(id)
	}
	return out
}

// permissionsHost is the host API behind card management, in the bindings'
// own types. The wasm build binds the generated calls (permissions_wasm.go);
// off the wasm target every call fails, and tests install a fake.
type permissionsHost struct {
	id               func(c *types.PermissionCard) types.CardId
	parents          func(c *types.PermissionCard) []types.CardId
	expiresAt        func(c *types.PermissionCard) witTypes.Option[perm.Timestamp]
	inspect          func(c *types.PermissionCard) witTypes.Result[perm.CardView, perm.PermissionError]
	derive           func(parent *types.PermissionCard, lp, ln, up, un []perm.PatternGrant, lifespan witTypes.Option[perm.Duration]) witTypes.Result[*types.PermissionCard, perm.PermissionError]
	deriveFromWallet func(lp, ln, up, un []perm.PatternGrant, lifespan witTypes.Option[perm.Duration]) witTypes.Result[*types.PermissionCard, perm.PermissionError]
	deriveScope      func(parents []*types.PermissionCard, lp, ln, up, un []perm.PatternGrant) witTypes.Result[*types.PermissionCard, perm.PermissionError]
	revoke           func(c *types.PermissionCard) witTypes.Result[uint32, perm.PermissionError]
	install          func(c *types.PermissionCard, target perm.Holder) witTypes.Result[witTypes.Unit, perm.PermissionError]
	wallet           func() []*types.PermissionCard
	walletVersion    func() perm.WalletVersionToken
	listModules      func() []KernelModule
	validateGrant    func(g perm.PatternGrant) witTypes.Result[witTypes.Unit, perm.PermissionError]
	release          func(c *types.PermissionCard)
}

// borrow returns the card's handle, leaving it with the card.
func (st *permissionCardState) borrow() *types.PermissionCard {
	if st == nil || st.h == nil {
		panic(ErrPermissionCardMoved)
	}
	return st.h
}

// take moves the handle out of the card, which is unusable afterwards.
func (st *permissionCardState) take() *types.PermissionCard {
	h := st.borrow()
	st.h = nil
	return h
}

func newPermissionCard(h *types.PermissionCard) PermissionCard {
	return PermissionCard{st: &permissionCardState{h: h}}
}

func cardResult(r witTypes.Result[*types.PermissionCard, perm.PermissionError]) (PermissionCard, error) {
	if r.IsErr() {
		return PermissionCard{}, permissionErrorFromWit(r.Err())
	}
	return newPermissionCard(r.Ok()), nil
}

func permissionUnitResult(r witTypes.Result[witTypes.Unit, perm.PermissionError]) error {
	if r.IsErr() {
		return permissionErrorFromWit(r.Err())
	}
	return nil
}

func (st *permissionCardState) id() CardID {
	return cardIDFromWit(permissionsHostOps.id(st.borrow()))
}

func (st *permissionCardState) parents() []CardID {
	return cardIDsFromWit(permissionsHostOps.parents(st.borrow()))
}

func (st *permissionCardState) expiresAt() Option[time.Time] {
	ts := permissionsHostOps.expiresAt(st.borrow())
	if ts.IsNone() {
		return None[time.Time]()
	}
	return Some(time.Unix(ts.Some().Seconds, int64(ts.Some().Nanoseconds)).UTC())
}

func (st *permissionCardState) inspect() (CardView, error) {
	r := permissionsHostOps.inspect(st.borrow())
	if r.IsErr() {
		return CardView{}, permissionErrorFromWit(r.Err())
	}
	v := r.Ok()
	return CardView{
		LowerPositive: grantsFromWit(v.LowerPositive),
		LowerNegative: grantsFromWit(v.LowerNegative),
		UpperPositive: grantsFromWit(v.UpperPositive),
		UpperNegative: grantsFromWit(v.UpperNegative),
	}, nil
}

func (st *permissionCardState) derive(b Bounds, lifespan Option[Lifespan]) (PermissionCard, error) {
	return cardResult(permissionsHostOps.derive(st.borrow(),
		grantsToWit(b.LowerPositive), grantsToWit(b.LowerNegative),
		grantsToWit(b.UpperPositive), grantsToWit(b.UpperNegative),
		lifespanToWit(lifespan)))
}

// revoke moves the card before the call: the host drops the handle whether or
// not the revocation succeeds.
func (st *permissionCardState) revoke() (uint32, error) {
	r := permissionsHostOps.revoke(st.take())
	if r.IsErr() {
		return 0, permissionErrorFromWit(r.Err())
	}
	return r.Ok(), nil
}

// install moves the card before the call: the host drops the handle whether
// or not the installation succeeds.
func (st *permissionCardState) install(target Holder) error {
	if !target.ok {
		panic(errors.New("golem: Install: a zero Holder; build one with AgentHolder, AccountHolder or AppHolder"))
	}
	return permissionUnitResult(permissionsHostOps.install(st.take(), target.wit))
}

func (st *permissionCardState) release() {
	if st == nil || st.h == nil {
		return
	}
	permissionsHostOps.release(st.take())
}

// ID returns the card's identifier. It traps when the host no longer knows the
// card.
func (c PermissionCard) ID() CardID { return c.st.id() }

// Parents returns the identifiers of the cards this card was derived from. It
// traps when the host no longer knows the card.
func (c PermissionCard) Parents() []CardID { return c.st.parents() }

// ExpiresAt returns when the card expires, or None for a card without a
// bounded lifespan. It traps when the host no longer knows the card.
func (c PermissionCard) ExpiresAt() Option[time.Time] { return c.st.expiresAt() }

// Inspect returns the card's grants. It needs the card:inspect verb.
func (c PermissionCard) Inspect() (CardView, error) { return c.st.inspect() }

// MustInspect is [PermissionCard.Inspect] that panics on a refusal.
func (c PermissionCard) MustInspect() CardView { return Must(c.st.inspect()) }

// Derive derives a child card bounded by this one: it keeps the positive
// grants of b and adds the negative ones, and lives for lifespan, or as long
// as this card when None. It needs the card:derive verb.
func (c PermissionCard) Derive(b Bounds, lifespan Option[Lifespan]) (PermissionCard, error) {
	return c.st.derive(b, lifespan)
}

// MustDerive is [PermissionCard.Derive] that panics on a refusal.
func (c PermissionCard) MustDerive(b Bounds, lifespan Option[Lifespan]) PermissionCard {
	return Must(c.st.derive(b, lifespan))
}

// Revoke revokes the card and every card derived from it, returning how many
// cards were revoked. It needs the card:revoke verb and a wallet card that is
// this card or one of its ancestors. The card is moved, even on an error.
func (c PermissionCard) Revoke() (uint32, error) { return c.st.revoke() }

// Install installs the card on target. It needs the card:install verb for
// target, and target must match one of the card's recipients. The card is
// moved, even on an error.
func (c PermissionCard) Install(target Holder) error { return c.st.install(target) }

// Release gives the card's handle back to the host; the card is moved
// afterwards. Releasing a moved card does nothing.
func (c PermissionCard) Release() { c.st.release() }

// ID returns the card's identifier. It traps when the host no longer knows the
// card.
func (c PolymorphicPermissionCard) ID() CardID { return c.st.id() }

// Parents returns the identifiers of the cards this card was derived from. It
// traps when the host no longer knows the card.
func (c PolymorphicPermissionCard) Parents() []CardID { return c.st.parents() }

// ExpiresAt returns when the card expires, or None for a card without a
// bounded lifespan. It traps when the host no longer knows the card.
func (c PolymorphicPermissionCard) ExpiresAt() Option[time.Time] { return c.st.expiresAt() }

// Inspect returns the card's grants. It needs the card:inspect verb.
func (c PolymorphicPermissionCard) Inspect() (CardView, error) { return c.st.inspect() }

// MustInspect is [PolymorphicPermissionCard.Inspect] that panics on a refusal.
func (c PolymorphicPermissionCard) MustInspect() CardView { return Must(c.st.inspect()) }

// Derive derives a child card bounded by this one, as [PermissionCard.Derive]
// does. A template card is first resolved for this agent, so the child is
// concrete; this needs the card to be in the agent's wallet.
func (c PolymorphicPermissionCard) Derive(b Bounds, lifespan Option[Lifespan]) (PermissionCard, error) {
	return c.st.derive(b, lifespan)
}

// MustDerive is [PolymorphicPermissionCard.Derive] that panics on a refusal.
func (c PolymorphicPermissionCard) MustDerive(b Bounds, lifespan Option[Lifespan]) PermissionCard {
	return Must(c.st.derive(b, lifespan))
}

// Revoke revokes the card and every card derived from it, as
// [PermissionCard.Revoke] does. The card is moved, even on an error.
func (c PolymorphicPermissionCard) Revoke() (uint32, error) { return c.st.revoke() }

// Install installs the card on target, as [PermissionCard.Install] does; a
// template card is resolved for target. The card is moved, even on an error.
func (c PolymorphicPermissionCard) Install(target Holder) error { return c.st.install(target) }

// Release gives the card's handle back to the host; the card is moved
// afterwards. Releasing a moved card does nothing.
func (c PolymorphicPermissionCard) Release() { c.st.release() }

// Wallet returns the cards installed in the agent's wallet, and the
// invocation's scope card when it has one. A wallet card may be a template, so
// each is a [PolymorphicPermissionCard]. Each is a fresh handle: release the
// ones you do not keep with [PolymorphicPermissionCard.Release] or
// [ReleaseCards].
func Wallet() []PolymorphicPermissionCard {
	hs := permissionsHostOps.wallet()
	out := make([]PolymorphicPermissionCard, len(hs))
	for i, h := range hs {
		out[i] = PolymorphicPermissionCard{st: &permissionCardState{h: h}}
	}
	return out
}

// ReleaseCards releases each card, as [AnyPermissionCard.Release] does.
func ReleaseCards[C AnyPermissionCard](cards ...C) {
	for _, c := range cards {
		c.Release()
	}
}

// WalletVersion returns the current version of the agent's wallet.
func WalletVersion() WalletVersionToken {
	v := permissionsHostOps.walletVersion()
	return WalletVersionToken{WalletIDHash: v.WalletIdHash, Generation: v.Generation}
}

// DeriveFromWallet derives a card with the lattice b from a card in the
// agent's wallet that covers it, living for lifespan. It needs the card:derive
// verb.
func DeriveFromWallet(b Bounds, lifespan Option[Lifespan]) (PermissionCard, error) {
	return cardResult(permissionsHostOps.deriveFromWallet(
		grantsToWit(b.LowerPositive), grantsToWit(b.LowerNegative),
		grantsToWit(b.UpperPositive), grantsToWit(b.UpperNegative),
		lifespanToWit(lifespan)))
}

// MustDeriveFromWallet is [DeriveFromWallet] that panics on a refusal.
func MustDeriveFromWallet(b Bounds, lifespan Option[Lifespan]) PermissionCard {
	return Must(DeriveFromWallet(b, lifespan))
}

// DeriveScope derives a scope card with the lattice b from parents. A scope
// card lives for the current invocation only and is never stored; attach it to
// an awaited call with [MethodDef.WithScopeCard] or [Client.WithScopeCard] to
// lend its grants to the callee for that call. It needs the card:derive verb.
func DeriveScope(b Bounds, parents ...AnyPermissionCard) (PermissionCard, error) {
	hs := make([]*types.PermissionCard, len(parents))
	for i, p := range parents {
		hs[i] = p.cardState().borrow()
	}
	return cardResult(permissionsHostOps.deriveScope(hs,
		grantsToWit(b.LowerPositive), grantsToWit(b.LowerNegative),
		grantsToWit(b.UpperPositive), grantsToWit(b.UpperNegative)))
}

// MustDeriveScope is [DeriveScope] that panics on a refusal.
func MustDeriveScope(b Bounds, parents ...AnyPermissionCard) PermissionCard {
	return Must(DeriveScope(b, parents...))
}

// ListKernelModules returns the resource classes the permission kernel knows,
// with their verbs.
func ListKernelModules() []KernelModule { return permissionsHostOps.listModules() }

// ValidateGrant checks that g names a known class, verb, owner, recipient and
// resource pattern.
func ValidateGrant(g PatternGrant) error {
	return permissionUnitResult(permissionsHostOps.validateGrant(grantsToWit([]PatternGrant{g})[0]))
}

// scopeCardHandle borrows the handle of an awaited call's scope card.
func scopeCardHandle(card AnyPermissionCard) witTypes.Option[*types.PermissionCard] {
	if card == nil {
		return witTypes.None[*types.PermissionCard]()
	}
	return witTypes.Some(card.cardState().borrow())
}
