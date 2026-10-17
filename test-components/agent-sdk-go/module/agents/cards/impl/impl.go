// Package impl is the IMPLEMENTATION of the permission-card scenario.
package impl

import (
	"errors"
	"fmt"
	"slices"
	"strings"
	"time"

	"agent-sdk-go/agents/cards"

	core "github.com/golemcloud/golem/sdks/go/core/schema"
	"github.com/golemcloud/golem/sdks/go/golem"
	"github.com/golemcloud/golem/sdks/go/golem/reflection"
)

type issuerState struct {
	// receiver and stale are what Scenario leaves for UseStaleScope.
	receiver string
	stale    golem.ScopeCard
}

type receiverState struct{ log []string }

var issuer = cards.Issuer.Implement(func(cards.IssuerId) *issuerState { return &issuerState{} })

var receiver = cards.Receiver.Implement(func(cards.ReceiverId) *receiverState { return &receiverState{} })

// errorKind is the permission-error kind of err, "remote-" and the kind of a
// failed call, or the message.
func errorKind(err error) string {
	if err == nil {
		return "ok"
	}
	var pe *golem.PermissionError
	if errors.As(err, &pe) {
		return pe.Kind.String()
	}
	var re *golem.RemoteCallError
	if errors.As(err, &re) {
		return "remote-" + re.Kind.String()
	}
	return err.Error()
}

// moved reports whether using the card panics as a moved card.
func moved(card interface{ ID() golem.CardID }) (isMoved bool) {
	defer func() {
		if r := recover(); r != nil {
			err, _ := r.(error)
			isMoved = errors.Is(err, golem.ErrPermissionCardMoved)
		}
	}()
	card.ID()
	return false
}

// inspectGrant finds the wallet card granting card inspection, and that
// grant, releasing the other wallet cards.
func inspectGrant() (golem.WalletCard, golem.PatternGrant, bool) {
	wallet := golem.Wallet()
	for i, c := range wallet {
		view, err := c.Inspect()
		if err != nil {
			continue
		}
		for _, g := range view.LowerPositive {
			if g.Class == "card" && g.Verb == "inspect" && g.ResourceID == "*" {
				golem.ReleaseCards(slices.Delete(slices.Clone(wallet), i, i+1)...)
				return c, g, true
			}
		}
	}
	golem.ReleaseCards(wallet...)
	return golem.WalletCard{}, golem.PatternGrant{}, false
}

// walletKinds lists the kind of each wallet card, sorted, with the scope
// card's expiry.
func walletKinds() []string {
	wallet := golem.Wallet()
	defer golem.ReleaseCards(wallet...)
	out := make([]string, len(wallet))
	for i, c := range wallet {
		out[i] = c.Kind().String()
		if s, ok := c.Scope(); ok {
			out[i] += "@" + expiry(s.ExpiresAt())
		}
	}
	slices.Sort(out)
	return out
}

func expiry(at golem.Option[time.Time]) string {
	t, ok := at.Get()
	if !ok {
		return "none"
	}
	return t.UTC().Format(time.RFC3339Nano)
}

// observe reports what the receiver's wallet lets it do.
func observe() string {
	wallet := golem.Wallet()
	defer golem.ReleaseCards(wallet...)
	result := "empty"
	for _, c := range wallet {
		_, err := c.Inspect()
		if err == nil {
			return "ok"
		}
		result = errorKind(err)
	}
	return result
}

func init() {
	issuer.Handle(cards.Scenario, func(ctx *golem.Context[issuerState], in cards.ScenarioIn) []string {
		var out []string
		note := func(key string, value any) { out = append(out, fmt.Sprintf("%s=%v", key, value)) }

		source, grant, found := inspectGrant()
		note("wallet-grants-inspect", found)
		if !found {
			return out
		}
		note("wallet-kinds", strings.Join(walletKinds(), ","))
		only := golem.Bounds{LowerPositive: []golem.PatternGrant{grant}}

		// 1. Derive a card from the wallet, and 2. inspect it.
		fromWallet := golem.MustDeriveFromWallet(only, golem.None[golem.Lifespan]())
		note("from-wallet-parents", len(fromWallet.Parents()))
		view := fromWallet.MustInspect()
		note("from-wallet-grants", fmt.Sprintf("%d/%d/%d/%d", len(view.LowerPositive), len(view.LowerNegative), len(view.UpperPositive), len(view.UpperNegative)))
		note("from-wallet-grant-is-requested", len(view.LowerPositive) == 1 && view.LowerPositive[0] == grant)
		note("from-wallet-expires", fromWallet.ExpiresAt().IsSome())

		// 3. Derive children: one with a lifespan, and one the revocation
		// reaches while this agent still holds it.
		now := time.Now()
		child := fromWallet.MustDerive(only, golem.Some(golem.LifespanOf(time.Hour)))
		note("child-parent-is-from-wallet", slices.Equal(child.Parents(), []golem.CardID{fromWallet.ID()}))
		expires, ok := child.ExpiresAt().Get()
		note("child-expires-within-the-hour", ok && expires.After(now.Add(59*time.Minute)) && !expires.After(now.Add(61*time.Minute)))
		kept := fromWallet.MustDerive(only, golem.Some(golem.MustParseLifespan("PT2H")))

		// 4. Lend scope cards to awaited calls; the receiver holds their
		// grants for those invocations only. A card attached to the call
		// takes precedence over the client's default.
		recv := cards.Receiver.Get(cards.ReceiverId{Name: in.Receiver})
		note("receiver-before", cards.Observe.MustCall(recv, golem.Unit{}))
		scope := golem.MustDeriveScope(only, source)
		empty := golem.MustDeriveScope(golem.Bounds{}, scope)
		note("scope-parent-is-source", slices.Equal(scope.Parents(), []golem.CardID{source.ID()}))
		note("receiver-with-scope", cards.Observe.WithScopeCard(scope).MustCall(recv, golem.Unit{}))
		note("receiver-wallet-with-scope", strings.Join(cards.WalletKinds.WithScopeCard(scope).MustCall(recv, golem.Unit{}), ","))
		note("receiver-with-default-scope", cards.Observe.CallAsync(recv.WithScopeCard(scope), golem.Unit{}).MustGet())
		note("receiver-call-scope-over-default", cards.Observe.WithScopeCard(empty).MustCall(recv.WithScopeCard(scope), golem.Unit{}))
		note("receiver-call-scope-over-empty-default", cards.Observe.WithScopeCard(scope).MustCall(recv.WithScopeCard(empty), golem.Unit{}))

		reflected, known := reflection.DiscoverAgentTypeByID(recv.AgentID())
		if !known {
			note("reflected", "no agent type")
			return out
		}
		rc := golem.Must(reflected.Bind(recv.AgentID()))
		reflectedObserve := func(c *reflection.AgentClient, opts ...reflection.CallOpt) any {
			got, _, err := c.Call("observe", map[string]any{}, opts...)
			if err != nil {
				return errorKind(err)
			}
			return got
		}
		note("reflected-default-scope", reflectedObserve(rc.WithScopeCard(scope)))
		note("reflected-call-scope-over-default", reflectedObserve(rc.WithScopeCard(scope), reflection.CallScopeCard(empty)))
		dc := golem.Must(reflection.BindAgentID(recv.AgentID()))
		method, _ := reflected.Method("observe")
		input := golem.Must(method.Input().PackJSON(map[string]any{}))
		dynamicObserve := func(c *reflection.DynamicAgentClient, opts ...reflection.CallOpt) string {
			got, _, err := c.Call("observe", input, opts...)
			if err != nil {
				return errorKind(err)
			}
			if s, ok := got.Unwrap().(core.StringValue); ok {
				return s.Value
			}
			return fmt.Sprintf("%#v", got)
		}
		note("dynamic-default-scope", dynamicObserve(dc.WithScopeCard(scope)))
		note("dynamic-call-scope-over-default", dynamicObserve(dc.WithScopeCard(empty), reflection.CallScopeCard(scope)))

		// Trigger and Schedule never carry the client's default scope card.
		withDefault := recv.WithScopeCard(scope)
		cards.Record.MustTrigger(withDefault, golem.Unit{})
		cards.Record.MustSchedule(withDefault, time.Now(), golem.Unit{})
		var logged []string
		for range 100 {
			if logged = cards.Log.MustCall(recv, golem.Unit{}); len(logged) >= 2 {
				break
			}
			time.Sleep(100 * time.Millisecond)
		}
		note("receiver-trigger-and-schedule", strings.Join(logged, ","))

		note("receiver-after-scope", cards.Observe.MustCall(recv, golem.Unit{}))
		note("scope-card-kept", !moved(scope))
		ctx.State.receiver, ctx.State.stale = in.Receiver, scope

		// 5. Install the child on the receiver.
		self := golem.MustGetSelfMetadata().AgentID
		target := golem.AgentHolder(golem.AgentID{ComponentID: self.ComponentID, AgentID: recv.AgentID()})
		note("install", errorKind(child.Install(target)))
		note("child-moved-by-install", moved(child))
		note("receiver-after-install", cards.Observe.MustCall(recv, golem.Unit{}))
		note("receiver-wallet-after-install", strings.Join(cards.WalletKinds.MustCall(recv, golem.Unit{}), ","))

		// 6. Revoke the wallet-derived card, and with it both children: the
		// receiver loses the authority of the installed one.
		revoked, err := fromWallet.Revoke()
		note("revoke", errorKind(err))
		note("revoked-count", revoked)
		note("from-wallet-moved-by-revoke", moved(fromWallet))
		note("receiver-after-revoke", cards.Observe.MustCall(recv, golem.Unit{}))
		kept.Release()

		// 7. Refusals.
		note("receiver-derive-from-wallet", cards.TryDeriveFromWallet.MustCall(recv, golem.Unit{}))
		elsewhere := grant
		elsewhere.Recipient = "nobody@example.com/*/*/*/*"
		mismatched := golem.MustDeriveFromWallet(golem.Bounds{LowerPositive: []golem.PatternGrant{elsewhere}}, golem.None[golem.Lifespan]())
		note("install-elsewhere", errorKind(mismatched.Install(target)))
		note("unknown-class", errorKind(golem.ValidateGrant(golem.PatternGrant{Class: "no-such-class", Owner: "*", Recipient: "*", Verb: "read", ResourceID: "*"})))

		source.Release()
		empty.Release()
		return out
	})
	issuer.Handle(cards.UseStaleScope, func(ctx *golem.Context[issuerState], _ golem.Unit) []string {
		recv := cards.Receiver.Get(cards.ReceiverId{Name: ctx.State.receiver})
		_, err := cards.Observe.WithScopeCard(ctx.State.stale).Call(recv, golem.Unit{})
		out := []string{"attach=" + errorKind(err)}
		_, err = golem.DeriveScope(golem.Bounds{}, ctx.State.stale)
		out = append(out, "derive-from="+errorKind(err))
		ctx.State.stale.Release()
		return append(out, "released="+fmt.Sprint(moved(ctx.State.stale)))
	})

	receiver.Handle(cards.Observe, func(_ *golem.Context[receiverState], _ golem.Unit) string {
		return observe()
	})
	receiver.Handle(cards.WalletKinds, func(_ *golem.Context[receiverState], _ golem.Unit) []string {
		return walletKinds()
	})
	receiver.Handle(cards.Record, func(ctx *golem.Context[receiverState], _ golem.Unit) golem.Unit {
		ctx.State.log = append(ctx.State.log, observe())
		return golem.Unit{}
	})
	receiver.Handle(cards.Log, func(ctx *golem.Context[receiverState], _ golem.Unit) []string {
		return ctx.State.log
	})
	receiver.Handle(cards.TryDeriveFromWallet, func(_ *golem.Context[receiverState], _ golem.Unit) string {
		card, err := golem.DeriveFromWallet(golem.Bounds{
			LowerPositive: []golem.PatternGrant{{Class: "card", Owner: "*", Recipient: "*", Verb: "inspect", ResourceID: "*"}},
		}, golem.None[golem.Lifespan]())
		if err == nil {
			card.Release()
		}
		return errorKind(err)
	})
}
