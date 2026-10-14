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
	"encoding/binary"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"math"
	"strconv"
	"strings"
	"time"
	"unique"

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
//
// A [ScopeCard] from [DeriveScope] lends grants to awaited calls for the
// current invocation only; see [MethodDef.WithScopeCard].

// ErrPermissionsOutsideComponent reports a card operation outside a component,
// where there is no host to perform it.
var ErrPermissionsOutsideComponent = errors.New("golem: permission cards are only available inside a component")

// AnyPermissionCard is a [PermissionCard] or a [PolymorphicPermissionCard], or a
// struct that embeds one and so has its methods. Nothing outside the package
// can implement it otherwise, and a [ScopeCard] does not.
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

// ScopeParent is a card [DeriveScope] accepts as a parent: a
// [PermissionCard], a [PolymorphicPermissionCard] or a [ScopeCard], or a
// struct that embeds one, which stands for the embedded card. Nothing outside
// the package can implement it otherwise.
type ScopeParent interface {
	cardState() *permissionCardState
}

var (
	_ AnyPermissionCard = PermissionCard{}
	_ AnyPermissionCard = PolymorphicPermissionCard{}
	_ ScopeParent       = PermissionCard{}
	_ ScopeParent       = PolymorphicPermissionCard{}
	_ ScopeParent       = ScopeCard{}
)

// CardID identifies a card. It is opaque: it can be compared, and used as a
// map key, to tell whether a card's [AnyPermissionCard.Parents] name another
// card, and nothing else. Formatting and serializing it show a placeholder,
// never the identifier.
type CardID struct{ h unique.Handle[string] }

// CardID holds the identifier's sixteen bytes as a string behind a canonical
// pointer: comparison still compares identifiers, while fmt, walking a struct
// that holds a CardID without calling its methods, reaches only a pointer to a
// scalar, which it prints as an address.

// redactedCardID is what every rendering of a [CardID] shows.
const redactedCardID = "golem.CardID{…}"

// String returns a placeholder, never the identifier.
func (CardID) String() string { return redactedCardID }

// GoString returns a placeholder, never the identifier.
func (CardID) GoString() string { return redactedCardID }

// Format writes a placeholder for every verb, never the identifier.
func (CardID) Format(f fmt.State, _ rune) { _, _ = io.WriteString(f, redactedCardID) }

// MarshalJSON writes the placeholder as a JSON string.
func (CardID) MarshalJSON() ([]byte, error) { return json.Marshal(redactedCardID) }

// MarshalText writes the placeholder.
func (CardID) MarshalText() ([]byte, error) { return []byte(redactedCardID), nil }

func cardIDFromWit(w types.CardId) CardID {
	var b [16]byte
	binary.BigEndian.PutUint64(b[:8], w.Uuid.HighBits)
	binary.BigEndian.PutUint64(b[8:], w.Uuid.LowBits)
	return CardID{h: unique.Make(string(b[:]))}
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
//
// A grant may use slot variables such as ?agent or ?env, which the host
// resolves for the calling agent: a grant [PolymorphicPermissionCard.Inspect]
// returned for a template card can be requested unchanged.
type Bounds struct {
	LowerPositive []PatternGrant
	LowerNegative []PatternGrant
	UpperPositive []PatternGrant
	UpperNegative []PatternGrant
}

// Lifespan is how long a derived card lives: a positive duration in the
// ISO-8601 form PnDTnHnMnS.
type Lifespan struct{ iso string }

// ParseLifespan reads a positive duration of days, hours, minutes and seconds
// in the ISO-8601 form PnDTnHnMnS, such as "PT30M", "P2D" or "P1DT12H1.5S".
// Each part is optional but there must be one, only the seconds may have a
// fraction, of at most nine digits, and the whole must fit in an int64 of
// nanoseconds. Calendar units — years, months and weeks — have no fixed
// length, and the host does not accept them.
func ParseLifespan(iso string) (Lifespan, error) {
	if err := checkLifespan(iso); err != nil {
		return Lifespan{}, fmt.Errorf("golem: lifespan %q: %w", iso, err)
	}
	return Lifespan{iso: iso}, nil
}

// MustParseLifespan is [ParseLifespan] that panics on a malformed duration.
func MustParseLifespan(iso string) Lifespan { return Must(ParseLifespan(iso)) }

// checkLifespan accepts P[nD][T[nH][nM][n[.f]S]] with at least one part, a
// fraction of at most nine digits, and a total of nanoseconds that is not zero
// and fits in an int64.
func checkLifespan(iso string) error {
	rest, ok := strings.CutPrefix(iso, "P")
	if !ok {
		return errors.New("not an ISO-8601 duration starting with 'P'")
	}
	date, clock, hasT := strings.Cut(rest, "T")
	if hasT && clock == "" {
		return errors.New("no hours, minutes or seconds after 'T'")
	}
	var total int64
	parts := 0
	add := func(n, unit int64) error {
		if n > (math.MaxInt64-total)/unit {
			return errors.New("longer than an int64 of nanoseconds")
		}
		total += n * unit
		return nil
	}
	take := func(s string, units string) error {
		for s != "" {
			i := 0
			for i < len(s) && (s[i] >= '0' && s[i] <= '9' || s[i] == '.') {
				i++
			}
			if i == len(s) {
				return fmt.Errorf("%q has no unit", s)
			}
			number, unit := s[:i], s[i]
			at := strings.IndexByte(units, unit)
			if at < 0 {
				return fmt.Errorf("unit %q is not one of %s here, or is out of order", unit, units)
			}
			units = units[at+1:]
			whole, frac, hasDot := strings.Cut(number, ".")
			if whole == "" || hasDot && (frac == "" || unit != 'S') || strings.Contains(frac, ".") {
				return fmt.Errorf("%q is not a number of %c", number, unit)
			}
			if len(frac) > 9 {
				return fmt.Errorf("%q has more than nine fractional digits", number)
			}
			n, err := strconv.ParseInt(whole, 10, 64)
			if err != nil {
				return errors.New("longer than an int64 of nanoseconds")
			}
			if err := add(n, unitNanos[unit]); err != nil {
				return err
			}
			if frac != "" {
				ns, _ := strconv.ParseInt(frac+strings.Repeat("0", 9-len(frac)), 10, 64)
				if err := add(ns, 1); err != nil {
					return err
				}
			}
			parts++
			s = s[i+1:]
		}
		return nil
	}
	if err := take(date, "D"); err != nil {
		return err
	}
	if err := take(clock, "HMS"); err != nil {
		return err
	}
	if parts == 0 {
		return errors.New("no duration after 'P'")
	}
	if total == 0 {
		return errors.New("a card lifespan must be positive")
	}
	return nil
}

// unitNanos is the length of each lifespan unit in nanoseconds.
var unitNanos = map[byte]int64{
	'D': int64(24 * time.Hour), 'H': int64(time.Hour), 'M': int64(time.Minute), 'S': int64(time.Second),
}

// LifespanOf is a lifespan of d. It panics when d is zero or negative.
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
func (c PolymorphicPermissionCard) ID() CardID { return c.pst.id() }

// Parents returns the identifiers of the cards this card was derived from. It
// traps when the host no longer knows the card.
func (c PolymorphicPermissionCard) Parents() []CardID { return c.pst.parents() }

// ExpiresAt returns when the card expires, or None for a card without a
// bounded lifespan. It traps when the host no longer knows the card.
func (c PolymorphicPermissionCard) ExpiresAt() Option[time.Time] { return c.pst.expiresAt() }

// Inspect returns the card's grants. It needs the card:inspect verb.
func (c PolymorphicPermissionCard) Inspect() (CardView, error) { return c.pst.inspect() }

// MustInspect is [PolymorphicPermissionCard.Inspect] that panics on a refusal.
func (c PolymorphicPermissionCard) MustInspect() CardView { return Must(c.pst.inspect()) }

// Derive derives a child card bounded by this one, as [PermissionCard.Derive]
// does. A template card is first resolved for this agent, so the child is
// concrete; this needs the card to be in the agent's wallet.
func (c PolymorphicPermissionCard) Derive(b Bounds, lifespan Option[Lifespan]) (PermissionCard, error) {
	return c.pst.derive(b, lifespan)
}

// MustDerive is [PolymorphicPermissionCard.Derive] that panics on a refusal.
func (c PolymorphicPermissionCard) MustDerive(b Bounds, lifespan Option[Lifespan]) PermissionCard {
	return Must(c.pst.derive(b, lifespan))
}

// Revoke revokes the card and every card derived from it, as
// [PermissionCard.Revoke] does. The card is moved, even on an error.
func (c PolymorphicPermissionCard) Revoke() (uint32, error) { return c.pst.revoke() }

// Install installs the card on target, as [PermissionCard.Install] does; a
// template card is resolved for target. The card is moved, even on an error.
func (c PolymorphicPermissionCard) Install(target Holder) error { return c.pst.install(target) }

// Release gives the card's handle back to the host; the card is moved
// afterwards. Releasing a moved card does nothing.
func (c PolymorphicPermissionCard) Release() { c.pst.release() }

// Wallet returns the cards installed in the agent's wallet, and the
// invocation's scope card when it has one and the agent holds card:inspect. A wallet card may be a template, so
// each is a [PolymorphicPermissionCard]; send one only where a
// PolymorphicPermissionCard is expected. To hand on a [PermissionCard], derive
// a concrete child with [PolymorphicPermissionCard.Derive]. The host refuses
// to send, install or revoke the scope card among them. Each is a fresh
// handle: release the ones you do not keep with
// [PolymorphicPermissionCard.Release] or [ReleaseCards].
func Wallet() []PolymorphicPermissionCard {
	hs := permissionsHostOps.wallet()
	out := make([]PolymorphicPermissionCard, len(hs))
	for i, h := range hs {
		out[i] = PolymorphicPermissionCard{pst: &permissionCardState{h: h}}
	}
	return out
}

// ReleaseCards releases each card, as [PermissionCard.Release] does.
func ReleaseCards[C interface {
	ScopeParent
	Release()
}](cards ...C) {
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

// DeriveScope derives a scope card with the lattice b from parents, which may
// be persistent cards and scope cards. It needs the card:derive verb.
//
// A scope card is valid only during the invocation that derived it. Attach it
// to the awaited calls this invocation makes, with [MethodDef.WithScopeCard] or
// [Client.WithScopeCard], to lend its grants to each callee for that call. Do
// not keep it, or a client that carries it, for a later invocation: once its
// invocation ends the host retires it, denies an awaited call that carries it,
// and refuses it as a DeriveScope parent with [PermissionNotPermitted].
// Releasing a retired scope card is safe.
func DeriveScope(b Bounds, parents ...ScopeParent) (ScopeCard, error) {
	hs := make([]*types.PermissionCard, len(parents))
	for i, p := range parents {
		hs[i] = p.cardState().borrow()
	}
	r := permissionsHostOps.deriveScope(hs,
		grantsToWit(b.LowerPositive), grantsToWit(b.LowerNegative),
		grantsToWit(b.UpperPositive), grantsToWit(b.UpperNegative))
	if r.IsErr() {
		return ScopeCard{}, permissionErrorFromWit(r.Err())
	}
	return ScopeCard{sst: &permissionCardState{h: r.Ok()}}, nil
}

// MustDeriveScope is [DeriveScope] that panics on a refusal.
func MustDeriveScope(b Bounds, parents ...ScopeParent) ScopeCard {
	return Must(DeriveScope(b, parents...))
}

// ID returns the scope card's identifier.
func (c ScopeCard) ID() CardID { return c.sst.id() }

// Parents returns the identifiers of the cards the scope card was derived
// from.
func (c ScopeCard) Parents() []CardID { return c.sst.parents() }

// ExpiresAt returns when the scope card expires, or None for a card without a
// bounded lifespan.
func (c ScopeCard) ExpiresAt() Option[time.Time] { return c.sst.expiresAt() }

// Inspect returns the scope card's grants. It needs the card:inspect verb.
func (c ScopeCard) Inspect() (CardView, error) { return c.sst.inspect() }

// MustInspect is [ScopeCard.Inspect] that panics on a refusal.
func (c ScopeCard) MustInspect() CardView { return Must(c.sst.inspect()) }

// Release gives the scope card's handle back to the host; the card is moved
// afterwards. Releasing a moved card does nothing.
func (c ScopeCard) Release() { c.sst.release() }

// ListKernelModules returns the resource classes the permission kernel knows,
// with their verbs.
func ListKernelModules() []KernelModule { return permissionsHostOps.listModules() }

// ValidateGrant checks that g names a known class, verb, owner, recipient and
// resource pattern.
func ValidateGrant(g PatternGrant) error {
	return permissionUnitResult(permissionsHostOps.validateGrant(grantsToWit([]PatternGrant{g})[0]))
}

// scopeCardHandle borrows the handle of an awaited call's scope card, None for
// the zero ScopeCard. It panics with [ErrPermissionCardMoved] on a released
// card.
func scopeCardHandle(card ScopeCard) witTypes.Option[*types.PermissionCard] {
	if card.sst == nil {
		return witTypes.None[*types.PermissionCard]()
	}
	return witTypes.Some(card.sst.borrow())
}

// mustScopeCard panics on the zero ScopeCard, which no attachment accepts.
func mustScopeCard(where string, card ScopeCard) ScopeCard {
	if card.sst == nil {
		panic(fmt.Errorf("golem: %s: WithScopeCard: a zero ScopeCard; derive one with DeriveScope", where))
	}
	return card
}
