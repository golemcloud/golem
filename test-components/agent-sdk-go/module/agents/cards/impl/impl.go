// Package impl is the IMPLEMENTATION of the permission-card scenario.
package impl

import (
	"errors"
	"fmt"
	"slices"
	"time"

	"agent-sdk-go/agents/cards"

	"github.com/golemcloud/golem/sdks/go/golem"
)

type state struct{}

var issuer = cards.Issuer.Implement(func(cards.IssuerId) *state { return &state{} })

var receiver = cards.Receiver.Implement(func(cards.ReceiverId) *state { return &state{} })

// errorKind is the permission-error kind of err, or its message.
func errorKind(err error) string {
	if err == nil {
		return "ok"
	}
	var pe *golem.PermissionError
	if errors.As(err, &pe) {
		return pe.Kind.String()
	}
	return err.Error()
}

// moved reports whether using the card panics as a moved card.
func moved(card golem.AnyPermissionCard) (isMoved bool) {
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
func inspectGrant() (golem.PolymorphicPermissionCard, golem.PatternGrant, bool) {
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
	return golem.PolymorphicPermissionCard{}, golem.PatternGrant{}, false
}

func init() {
	issuer.Handle(cards.Scenario, func(_ *golem.Context[state], in cards.ScenarioIn) []string {
		var out []string
		note := func(key string, value any) { out = append(out, fmt.Sprintf("%s=%v", key, value)) }

		source, grant, found := inspectGrant()
		note("wallet-grants-inspect", found)
		if !found {
			return out
		}
		only := golem.Bounds{LowerPositive: []golem.PatternGrant{grant}}

		// 1. Derive a card from the wallet, and 2. inspect it.
		fromWallet := golem.MustDeriveFromWallet(only, golem.None[golem.Lifespan]())
		note("from-wallet-parents", len(fromWallet.Parents()))
		view := fromWallet.MustInspect()
		note("from-wallet-grants", fmt.Sprintf("%d/%d/%d/%d", len(view.LowerPositive), len(view.LowerNegative), len(view.UpperPositive), len(view.UpperNegative)))
		note("from-wallet-grant-is-requested", len(view.LowerPositive) == 1 && view.LowerPositive[0] == grant)
		note("from-wallet-expires", fromWallet.ExpiresAt().IsSome())

		// 3. Derive a child with a lifespan.
		now := time.Now()
		child := fromWallet.MustDerive(only, golem.Some(golem.LifespanOf(time.Hour)))
		note("child-parent-is-from-wallet", slices.Equal(child.Parents(), []golem.CardID{fromWallet.ID()}))
		expires, ok := child.ExpiresAt().Get()
		note("child-expires-within-the-hour", ok && expires.After(now.Add(59*time.Minute)) && !expires.After(now.Add(61*time.Minute)))

		// 4. Lend a scope card to awaited calls; the receiver holds its grants
		// for those invocations only.
		recv := cards.Receiver.Get(cards.ReceiverId{Name: in.Receiver})
		note("receiver-before", cards.Observe.MustCall(recv, golem.Unit{}))
		scope := golem.MustDeriveScope(only, source)
		note("receiver-with-scope", cards.Observe.WithScopeCard(scope).MustCall(recv, golem.Unit{}))
		note("receiver-with-default-scope", cards.Observe.CallAsync(recv.WithScopeCard(scope), golem.Unit{}).MustGet())
		note("receiver-after-scope", cards.Observe.MustCall(recv, golem.Unit{}))
		note("scope-card-kept", !moved(scope))

		// 5. Install the child on the receiver.
		self := golem.MustGetSelfMetadata().AgentID
		target := golem.AgentHolder(golem.AgentID{ComponentID: self.ComponentID, AgentID: recv.AgentID()})
		note("install", errorKind(child.Install(target)))
		note("child-moved-by-install", moved(child))
		note("receiver-after-install", cards.Observe.MustCall(recv, golem.Unit{}))

		// 6. Revoke the wallet-derived card, and with it the installed child.
		revoked, err := fromWallet.Revoke()
		note("revoke", errorKind(err))
		note("revoked-count", revoked)
		note("from-wallet-moved-by-revoke", moved(fromWallet))
		note("receiver-after-revoke", cards.Observe.MustCall(recv, golem.Unit{}))

		// 7. Refusals.
		note("receiver-derive-from-wallet", cards.TryDeriveFromWallet.MustCall(recv, golem.Unit{}))
		elsewhere := grant
		elsewhere.Recipient = "nobody@example.com/*/*/*/*"
		mismatched := golem.MustDeriveFromWallet(golem.Bounds{LowerPositive: []golem.PatternGrant{elsewhere}}, golem.None[golem.Lifespan]())
		note("install-elsewhere", errorKind(mismatched.Install(target)))
		note("unknown-class", errorKind(golem.ValidateGrant(golem.PatternGrant{Class: "no-such-class", Owner: "*", Recipient: "*", Verb: "read", ResourceID: "*"})))

		source.Release()
		scope.Release()
		return out
	})

	receiver.Handle(cards.Observe, func(_ *golem.Context[state], _ golem.Unit) string {
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
	})
	receiver.Handle(cards.TryDeriveFromWallet, func(_ *golem.Context[state], _ golem.Unit) string {
		card, err := golem.DeriveFromWallet(golem.Bounds{
			LowerPositive: []golem.PatternGrant{{Class: "card", Owner: "*", Recipient: "*", Verb: "inspect", ResourceID: "*"}},
		}, golem.None[golem.Lifespan]())
		if err == nil {
			card.Release()
		}
		return errorKind(err)
	})
}
