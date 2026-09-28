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
	"fmt"
	"reflect"

	witTypes "go.bytecodealliance.org/pkg/wit/types"

	common "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_common"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
)

// Principal is the identity behind an invocation: an [OidcPrincipal], an
// [AgentPrincipal], a [GolemUserPrincipal] or an [AnonymousPrincipal].
//
// A method reads the principal of its invocation from a Principal field of its
// input struct, which the host fills; callers leave it nil and never send it:
//
//	type ChargeIn struct {
//	    Amount    int64
//	    Principal golem.Principal
//	}
//
// [Context.Principal] is the principal the agent was initialized with. A
// read-only method whose input has a Principal field caches per principal.
type Principal interface{ isPrincipal() }

// OidcPrincipal is a user authenticated through an OIDC provider.
type OidcPrincipal struct {
	Sub               string
	Issuer            string
	Email             Option[string]
	Name              Option[string]
	EmailVerified     Option[bool]
	GivenName         Option[string]
	FamilyName        Option[string]
	Picture           Option[string]
	PreferredUsername Option[string]
	// Claims is the token's full claim set, as JSON.
	Claims string
}

// AgentPrincipal is another agent, calling over RPC.
type AgentPrincipal struct {
	// ComponentID is the component the calling agent belongs to.
	ComponentID UUID
	// AgentID is the calling agent's string id (agent type + constructor
	// parameters).
	AgentID string
}

// GolemUserPrincipal is a Golem account, e.g. calling from the CLI.
type GolemUserPrincipal struct{ AccountID UUID }

// AnonymousPrincipal is an unauthenticated caller.
type AnonymousPrincipal struct{}

func (OidcPrincipal) isPrincipal()      {}
func (AgentPrincipal) isPrincipal()     {}
func (GolemUserPrincipal) isPrincipal() {}
func (AnonymousPrincipal) isPrincipal() {}

var principalType = reflect.TypeFor[Principal]()

func principalFromWit(p common.Principal) Principal {
	switch p.Tag() {
	case common.PrincipalOidc:
		o := p.Oidc()
		return OidcPrincipal{
			Sub: o.Sub, Issuer: o.Issuer,
			Email: optionFromWit(o.Email), Name: optionFromWit(o.Name),
			EmailVerified: optionFromWit(o.EmailVerified),
			GivenName:     optionFromWit(o.GivenName), FamilyName: optionFromWit(o.FamilyName),
			Picture: optionFromWit(o.Picture), PreferredUsername: optionFromWit(o.PreferredUsername),
			Claims: o.Claims,
		}
	case common.PrincipalAgent:
		a := p.Agent().AgentId
		return AgentPrincipal{ComponentID: uuidFromWit(a.ComponentId.Uuid), AgentID: a.AgentId}
	case common.PrincipalGolemUser:
		return GolemUserPrincipal{AccountID: uuidFromWit(p.GolemUser().AccountId.Uuid)}
	default:
		return AnonymousPrincipal{}
	}
}

func principalToWit(p Principal) common.Principal {
	switch p := p.(type) {
	case OidcPrincipal:
		return common.MakePrincipalOidc(common.OidcPrincipal{
			Sub: p.Sub, Issuer: p.Issuer,
			Email: optionToWit(p.Email), Name: optionToWit(p.Name),
			EmailVerified: optionToWit(p.EmailVerified),
			GivenName:     optionToWit(p.GivenName), FamilyName: optionToWit(p.FamilyName),
			Picture: optionToWit(p.Picture), PreferredUsername: optionToWit(p.PreferredUsername),
			Claims: p.Claims,
		})
	case AgentPrincipal:
		return common.MakePrincipalAgent(common.AgentPrincipal{AgentId: types.AgentId{
			ComponentId: types.ComponentId{Uuid: uuidToWit(p.ComponentID)},
			AgentId:     p.AgentID,
		}})
	case GolemUserPrincipal:
		return common.MakePrincipalGolemUser(common.GolemUserPrincipal{
			AccountId: types.AccountId{Uuid: uuidToWit(p.AccountID)},
		})
	default:
		return common.MakePrincipalAnonymous()
	}
}

func optionFromWit[T any](o witTypes.Option[T]) Option[T] {
	if o.IsSome() {
		return Some(o.Some())
	}
	return None[T]()
}

func optionToWit[T any](o Option[T]) witTypes.Option[T] {
	if v, ok := o.Get(); ok {
		return witTypes.Some(v)
	}
	return witTypes.None[T]()
}

// marshalPrincipal and unmarshalPrincipal carry a principal through a snapshot,
// in the host's own shape.
func marshalPrincipal(p Principal) ([]byte, error) {
	w := principalToWit(p)
	var dto principalDTO
	switch w.Tag() {
	case common.PrincipalOidc:
		o := w.Oidc()
		dto.Oidc = &oidcDTO{
			Sub: o.Sub, Issuer: o.Issuer, Claims: o.Claims,
			Email: ptr(o.Email), Name: ptr(o.Name), EmailVerified: ptr(o.EmailVerified),
			GivenName: ptr(o.GivenName), FamilyName: ptr(o.FamilyName),
			Picture: ptr(o.Picture), PreferredUsername: ptr(o.PreferredUsername),
		}
	case common.PrincipalAgent:
		a := p.(AgentPrincipal)
		dto.Agent = &a
	case common.PrincipalGolemUser:
		u := p.(GolemUserPrincipal)
		dto.GolemUser = &u
	}
	return json.Marshal(dto)
}

func unmarshalPrincipal(data []byte) (Principal, error) {
	var dto principalDTO
	if err := json.Unmarshal(data, &dto); err != nil {
		return nil, fmt.Errorf("golem: decoding the snapshot's principal: %w", err)
	}
	switch {
	case dto.Oidc != nil:
		o := dto.Oidc
		return OidcPrincipal{
			Sub: o.Sub, Issuer: o.Issuer, Claims: o.Claims,
			Email: opt(o.Email), Name: opt(o.Name), EmailVerified: opt(o.EmailVerified),
			GivenName: opt(o.GivenName), FamilyName: opt(o.FamilyName),
			Picture: opt(o.Picture), PreferredUsername: opt(o.PreferredUsername),
		}, nil
	case dto.Agent != nil:
		return *dto.Agent, nil
	case dto.GolemUser != nil:
		return *dto.GolemUser, nil
	default:
		return AnonymousPrincipal{}, nil
	}
}

type principalDTO struct {
	Oidc      *oidcDTO            `json:"oidc,omitempty"`
	Agent     *AgentPrincipal     `json:"agent,omitempty"`
	GolemUser *GolemUserPrincipal `json:"golemUser,omitempty"`
}

type oidcDTO struct {
	Sub               string  `json:"sub"`
	Issuer            string  `json:"issuer"`
	Email             *string `json:"email,omitempty"`
	Name              *string `json:"name,omitempty"`
	EmailVerified     *bool   `json:"emailVerified,omitempty"`
	GivenName         *string `json:"givenName,omitempty"`
	FamilyName        *string `json:"familyName,omitempty"`
	Picture           *string `json:"picture,omitempty"`
	PreferredUsername *string `json:"preferredUsername,omitempty"`
	Claims            string  `json:"claims"`
}

func ptr[T any](o witTypes.Option[T]) *T {
	if o.IsSome() {
		v := o.Some()
		return &v
	}
	return nil
}

func opt[T any](p *T) Option[T] {
	if p == nil {
		return None[T]()
	}
	return Some(*p)
}
