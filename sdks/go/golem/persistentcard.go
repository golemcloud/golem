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

import "time"

// PersistentCard is the host-tagged result of persistent derivation. Kind,
// Concrete and Template distinguish the result without inspecting its grants.
// Both views share the same affine handle: moving one moves all its copies.
// A PersistentCard cannot be sent directly; send its Concrete or Template view.
// Its zero value holds no card.
type PersistentCard struct {
	st   *permissionCardState
	kind WalletCardKind
}

// Kind returns WalletCardConcrete or WalletCardTemplate, or zero for no card.
func (c PersistentCard) Kind() WalletCardKind { return c.kind }

// Concrete returns the concrete card, if the host returned that case.
func (c PersistentCard) Concrete() (PermissionCard, bool) {
	if c.kind != WalletCardConcrete {
		return PermissionCard{}, false
	}
	return PermissionCard{st: c.st}, true
}

// Template returns the template card, if the host returned that case.
func (c PersistentCard) Template() (PolymorphicPermissionCard, bool) {
	if c.kind != WalletCardTemplate {
		return PolymorphicPermissionCard{}, false
	}
	return PolymorphicPermissionCard{pst: c.st}, true
}

func (c PersistentCard) cardState() *permissionCardState { return c.st }

// ID returns the card identifier.
func (c PersistentCard) ID() CardID { return c.st.id() }

// Parents returns the identifiers of the parent cards.
func (c PersistentCard) Parents() []CardID { return c.st.parents() }

// ExpiresAt returns the expiry, or None for an unbounded lifespan.
func (c PersistentCard) ExpiresAt() Option[time.Time] { return c.st.expiresAt() }

// Inspect returns the grants, requiring card:inspect.
func (c PersistentCard) Inspect() (CardView, error) { return c.st.inspect() }

// MustInspect is Inspect that panics on a refusal.
func (c PersistentCard) MustInspect() CardView { return Must(c.Inspect()) }

// Derive derives a narrower child, requiring card:derive. See Bounds.
func (c PersistentCard) Derive(b Bounds, lifespan Option[Lifespan]) (PersistentCard, error) {
	return c.st.derive(b, lifespan)
}

// MustDerive is Derive that panics on a refusal.
func (c PersistentCard) MustDerive(b Bounds, lifespan Option[Lifespan]) PersistentCard {
	return Must(c.Derive(b, lifespan))
}

// Install installs the card on target, consuming it even on failure.
func (c PersistentCard) Install(target Holder) error { return c.st.install(target) }

// Revoke revokes this card and its descendants, consuming it even on failure.
func (c PersistentCard) Revoke() (uint32, error) { return c.st.revoke() }

// Release releases the shared handle. Releasing a moved card does nothing.
func (c PersistentCard) Release() { c.st.release() }

// MarshalJSON refuses: a card is a host handle, not data.
func (PersistentCard) MarshalJSON() ([]byte, error) { return nil, errPermissionCardJSON }

// UnmarshalJSON refuses: no data turns into a card.
func (*PersistentCard) UnmarshalJSON([]byte) error { return errPermissionCardJSON }
