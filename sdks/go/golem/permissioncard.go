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
	"reflect"

	"github.com/golemcloud/golem/sdks/go/golem/internal/engine"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
)

// Permission cards.
//
// A permission card is delegated authority, received from the host or another
// call and handed on as a method or tool argument or result. It wraps a host
// handle: there is nothing to construct, and the host checks every use. A card
// is affine: sending a card moves it, and the copy that was sent is unusable
// afterwards, even when the call fails. Card management — deriving, inspecting,
// installing and revoking cards — is in permissions.go.
//
//	type ForwardIn struct{ Card golem.PermissionCard }
//
//	var _ = agent.Handle(Forward, func(_ *golem.Context[state], in ForwardIn) golem.PermissionCard {
//	    return Accept.MustCall(Receiver.Get(ReceiverID{Name: "target"}), AcceptIn{Card: in.Card})
//	})
//
// The two card types are not convertible into each other: a card travels as
// the type it was received or derived as. A [ScopeCard] is not a permission
// card at all: it cannot be sent, only attached to awaited calls.

// ErrPermissionCardMoved reports a card used after it was handed on.
var ErrPermissionCardMoved = errors.New("golem: the permission card was moved")

// PermissionCard is a card whose grants name concrete owners and resources.
type PermissionCard struct{ st *permissionCardState }

// PolymorphicPermissionCard is a card whose grants may leave the owner or the
// resource id open, to be filled in where it is used.
type PolymorphicPermissionCard struct{ pst *permissionCardState }

// ScopeCard is a card from [DeriveScope]. It is valid only during the
// invocation that derived it, and it cannot be sent, installed or revoked:
// attach it to awaited calls to lend its grants to the callees.
type ScopeCard struct{ sst *permissionCardState }

type permissionCardState struct{ h *types.PermissionCard }

// errPermissionCardJSON reports an attempt to serialize a card, or to build
// one from JSON: a card is a host handle, not data.
var errPermissionCardJSON = errors.New("golem: a permission card cannot be serialized; it is a host handle, not data")

// MarshalJSON refuses: a card is a host handle, not data.
func (PermissionCard) MarshalJSON() ([]byte, error) { return nil, errPermissionCardJSON }

// UnmarshalJSON refuses: no data turns into a card.
func (*PermissionCard) UnmarshalJSON([]byte) error { return errPermissionCardJSON }

// MarshalJSON refuses: a card is a host handle, not data.
func (PolymorphicPermissionCard) MarshalJSON() ([]byte, error) { return nil, errPermissionCardJSON }

// UnmarshalJSON refuses: no data turns into a card.
func (*PolymorphicPermissionCard) UnmarshalJSON([]byte) error { return errPermissionCardJSON }

// MarshalJSON refuses: a card is a host handle, not data.
func (ScopeCard) MarshalJSON() ([]byte, error) { return nil, errPermissionCardJSON }

// UnmarshalJSON refuses: no data turns into a card.
func (*ScopeCard) UnmarshalJSON([]byte) error { return errPermissionCardJSON }

func (c PermissionCard) cardState() *permissionCardState            { return c.st }
func (c PermissionCard) polymorphic() bool                          { return false }
func (c PolymorphicPermissionCard) cardState() *permissionCardState { return c.pst }
func (c PolymorphicPermissionCard) polymorphic() bool               { return true }
func (c ScopeCard) cardState() *permissionCardState                 { return c.sst }

// errScopeCardNotSendable is the definition error of a type that holds a
// [ScopeCard] where a value is sent.
const errScopeCardNotSendable = "golem.ScopeCard cannot be sent: it is valid only in the invocation that derived it; attach it to an awaited call with WithScopeCard"

// compilePermissionCard lowers a card type to the WIT permission-card type.
// Encoding moves the card, and gives it back when the encode fails before the
// host takes the value. Decoding adopts the received card and takes it out of
// the value, so no second decode of the same value can adopt it again.
func compilePermissionCard(c *engine.Codec, polymorphic bool) {
	c.Body = func(*engine.GraphBuilder) types.SchemaTypeBody {
		return types.MakeSchemaTypeBodyPermissionCardType(types.PermissionCardSpec{Polymorphic: polymorphic})
	}
	c.Encode = func(b *engine.ValBuilder, v reflect.Value) int32 {
		st := v.Interface().(AnyPermissionCard).cardState()
		if st == nil || st.h == nil {
			panic(&engine.EncodeError{Msg: ErrPermissionCardMoved.Error()})
		}
		h := st.h
		st.h = nil
		b.Moved(func() { st.h = h })
		return b.Push(types.MakeSchemaValueNodePermissionCardHandle(h))
	}
	c.Decode = func(d *engine.Decoder, dst reflect.Value, idx int32) error {
		n, err := d.Node(idx)
		if err != nil {
			return err
		}
		if n.Tag() != types.SchemaValueNodePermissionCardHandle {
			return fmt.Errorf("cannot decode value node (tag %d) into %s", n.Tag(), c.Typ)
		}
		h := n.PermissionCardHandle()
		if h == nil {
			return errPermissionCardTaken
		}
		d.Take(idx, types.MakeSchemaValueNodePermissionCardHandle(nil))
		st := &permissionCardState{h: h}
		if polymorphic {
			dst.Set(reflect.ValueOf(PolymorphicPermissionCard{pst: st}))
		} else {
			dst.Set(reflect.ValueOf(PermissionCard{st: st}))
		}
		return nil
	}
}

// errPermissionCardTaken reports a second decode of a card out of the same
// value: the first one already holds it.
var errPermissionCardTaken = errors.New("golem: the permission card in this value was already decoded")
