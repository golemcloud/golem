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
	"strings"
	"time"

	common "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_common"
)

// A [Caller] says who caused the running invocation. A method handler, a
// constructor, a tool or a tool middleware reads it with its context's Caller
// method:
//
//	func (ctx *golem.Context[S]) Caller() golem.Caller
//
// A Caller is a live value, not data: it has no schema and cannot be a method
// input or result, nor a field of one. Store ctx.Caller().Original.Ref() — a
// [PrincipalRef] — instead.
//
// The host is asked for the caller only when Caller is first called, and
// only during the call: a context kept past its call panics on a first read.

// PrincipalRef is the identity of one principal: an issuer and a subject.
//
// A reference is data: holding one grants nothing. Two references name the
// same principal exactly when both strings are equal, byte for byte. It is
// the named record principal-ref { issuer, subject } wherever it travels as a
// parameter, a result or part of stored state.
//
// Its text form, from [PrincipalRef.String] and [ParsePrincipalRef], is
// ("<issuer>","<subject>"), and the parser refuses every other spelling.
type PrincipalRef struct {
	Issuer  string
	Subject string
}

// principalRefTypeID is the schema type id every SDK gives a [PrincipalRef].
const principalRefTypeID = "principal-ref"

// Equals reports whether both references have the same issuer and subject.
func (r PrincipalRef) Equals(other PrincipalRef) bool { return r == other }

// String is the canonical text form ("<issuer>","<subject>").
func (r PrincipalRef) String() string {
	var b strings.Builder
	b.WriteByte('(')
	writeRefString(&b, r.Issuer)
	b.WriteByte(',')
	writeRefString(&b, r.Subject)
	b.WriteByte(')')
	return b.String()
}

// writeRefString writes s quoted, escaped as RFC 8785 serializes JSON strings.
func writeRefString(b *strings.Builder, s string) {
	b.WriteByte('"')
	for _, c := range s {
		switch {
		case c == '"':
			b.WriteString(`\"`)
		case c == '\\':
			b.WriteString(`\\`)
		case c == '\b':
			b.WriteString(`\b`)
		case c == '\t':
			b.WriteString(`\t`)
		case c == '\n':
			b.WriteString(`\n`)
		case c == '\f':
			b.WriteString(`\f`)
		case c == '\r':
			b.WriteString(`\r`)
		case c < 0x20:
			fmt.Fprintf(b, `\u%04x`, c)
		default:
			b.WriteRune(c)
		}
	}
	b.WriteByte('"')
}

// ParsePrincipalRef reads the canonical text form ("<issuer>","<subject>") and
// refuses every other spelling.
func ParsePrincipalRef(text string) (PrincipalRef, error) {
	p := refParser{text: []rune(text)}
	if err := p.expect('('); err != nil {
		return PrincipalRef{}, err
	}
	issuer, err := p.str()
	if err != nil {
		return PrincipalRef{}, err
	}
	if err := p.expect(','); err != nil {
		return PrincipalRef{}, err
	}
	subject, err := p.str()
	if err != nil {
		return PrincipalRef{}, err
	}
	if err := p.expect(')'); err != nil {
		return PrincipalRef{}, err
	}
	if p.pos < len(p.text) {
		return PrincipalRef{}, p.fail("trailing text")
	}
	return PrincipalRef{Issuer: issuer, Subject: subject}, nil
}

type refParser struct {
	text []rune
	pos  int
}

func (p *refParser) fail(what string) error {
	return fmt.Errorf("golem: invalid principal reference text: %s at character %d", what, p.pos)
}

func (p *refParser) expect(r rune) error {
	if p.pos < len(p.text) && p.text[p.pos] == r {
		p.pos++
		return nil
	}
	return p.fail(fmt.Sprintf("expected %q", r))
}

func (p *refParser) str() (string, error) {
	if err := p.expect('"'); err != nil {
		return "", err
	}
	var b strings.Builder
	for {
		if p.pos >= len(p.text) {
			return "", p.fail("unterminated string")
		}
		c := p.text[p.pos]
		p.pos++
		switch {
		case c == '"':
			return b.String(), nil
		case c == '\\':
			e, err := p.escape()
			if err != nil {
				return "", err
			}
			b.WriteRune(e)
		case c < 0x20:
			p.pos--
			return "", p.fail("unescaped control character")
		default:
			b.WriteRune(c)
		}
	}
}

// escape reads the escape after a backslash. Only the escapes that String
// writes are accepted, so every character has exactly one spelling.
func (p *refParser) escape() (rune, error) {
	if p.pos >= len(p.text) {
		return 0, p.fail("non-canonical escape")
	}
	c := p.text[p.pos]
	p.pos++
	switch c {
	case '"':
		return '"', nil
	case '\\':
		return '\\', nil
	case 'b':
		return '\b', nil
	case 't':
		return '\t', nil
	case 'n':
		return '\n', nil
	case 'f':
		return '\f', nil
	case 'r':
		return '\r', nil
	case 'u':
		if p.pos+4 > len(p.text) {
			return 0, p.fail("non-canonical escape")
		}
		var code rune
		for _, h := range p.text[p.pos : p.pos+4] {
			switch {
			case h >= '0' && h <= '9':
				code = code*16 + (h - '0')
			case h >= 'a' && h <= 'f':
				code = code*16 + (h - 'a' + 10)
			default:
				return 0, p.fail("non-canonical escape")
			}
		}
		if code >= 0x20 || code == 0x08 || code == 0x09 || code == 0x0a || code == 0x0c || code == 0x0d {
			return 0, p.fail("non-canonical escape")
		}
		p.pos += 4
		return code, nil
	default:
		return 0, p.fail("non-canonical escape")
	}
}

// PrincipalKind is the kind of a [Principal].
type PrincipalKind string

// The kinds of principal.
const (
	KindUser      PrincipalKind = "user"
	KindAccount   PrincipalKind = "account"
	KindAgent     PrincipalKind = "agent"
	KindTool      PrincipalKind = "tool"
	KindAnonymous PrincipalKind = "anonymous"
)

// Principal is one side of a call: a [UserPrincipal], an [AccountPrincipal], an
// [AgentPrincipal], a [ToolPrincipal] or an [AnonymousPrincipal].
//
// A principal is a live value, not data. It has no schema: store [Principal.Ref]
// instead.
type Principal interface {
	// Kind is the kind of this principal.
	Kind() PrincipalKind
	// Ref is the identity of this principal; ok is false for an anonymous one.
	Ref() (ref PrincipalRef, ok bool)
	// Equals reports whether other is the same principal. An anonymous
	// principal equals nothing.
	Equals(other Principal) bool
	// EqualsRef reports whether this principal is the one ref names.
	EqualsRef(ref PrincipalRef) bool
	// String is "<kind> <ref text>", or "anonymous". It never includes claims.
	String() string
	isPrincipal()
}

// UserPrincipal is a person who signed in through an identity provider.
type UserPrincipal struct {
	Pair PrincipalRef
	// Claims is the sign-in's claim set, as the exact JSON object text the host recorded.
	Claims string
	// AuthenticatedAt is when the user signed in.
	AuthenticatedAt time.Time
}

// AccountAuthentication is how an account proved itself.
type AccountAuthentication uint8

// The ways an account authenticates.
const (
	AccountAPIToken AccountAuthentication = iota
	AccountBootstrapToken
	AccountSignIn
)

// AccountPrincipal is a Golem account, seen through the CLI, the REST API or a token.
type AccountPrincipal struct {
	Pair           PrincipalRef
	AccountID      UUID
	Authentication AccountAuthentication
}

// AgentPrincipal is a running agent that made a call.
type AgentPrincipal struct {
	Pair          PrincipalRef
	AgentID       AgentID
	AgentType     string
	EnvironmentID UUID
}

// ToolPrincipal is a tool that made a call.
type ToolPrincipal struct {
	Pair          PrincipalRef
	EnvironmentID UUID
	ToolName      string
}

// AnonymousPrincipal is nobody: an unauthenticated caller. It has no
// reference and equals nothing.
type AnonymousPrincipal struct{}

func (UserPrincipal) isPrincipal()      {}
func (AccountPrincipal) isPrincipal()   {}
func (AgentPrincipal) isPrincipal()     {}
func (ToolPrincipal) isPrincipal()      {}
func (AnonymousPrincipal) isPrincipal() {}

// Kind is the kind of this principal.
func (UserPrincipal) Kind() PrincipalKind { return KindUser }

// Kind is the kind of this principal.
func (AccountPrincipal) Kind() PrincipalKind { return KindAccount }

// Kind is the kind of this principal.
func (AgentPrincipal) Kind() PrincipalKind { return KindAgent }

// Kind is the kind of this principal.
func (ToolPrincipal) Kind() PrincipalKind { return KindTool }

// Kind is the kind of this principal.
func (AnonymousPrincipal) Kind() PrincipalKind { return KindAnonymous }

// Ref is the identity of this principal; ok is false for an anonymous one.
func (p UserPrincipal) Ref() (PrincipalRef, bool) { return p.Pair, true }

// Ref is the identity of this principal; ok is false for an anonymous one.
func (p AccountPrincipal) Ref() (PrincipalRef, bool) { return p.Pair, true }

// Ref is the identity of this principal; ok is false for an anonymous one.
func (p AgentPrincipal) Ref() (PrincipalRef, bool) { return p.Pair, true }

// Ref is the identity of this principal; ok is false for an anonymous one.
func (p ToolPrincipal) Ref() (PrincipalRef, bool) { return p.Pair, true }

// Ref is the identity of this principal; ok is false for an anonymous one.
func (AnonymousPrincipal) Ref() (PrincipalRef, bool) { return PrincipalRef{}, false }

// Equals reports whether other is the same principal. An anonymous principal equals nothing.
func (p UserPrincipal) Equals(o Principal) bool { return principalEquals(p, o) }

// Equals reports whether other is the same principal. An anonymous principal equals nothing.
func (p AccountPrincipal) Equals(o Principal) bool { return principalEquals(p, o) }

// Equals reports whether other is the same principal. An anonymous principal equals nothing.
func (p AgentPrincipal) Equals(o Principal) bool { return principalEquals(p, o) }

// Equals reports whether other is the same principal. An anonymous principal equals nothing.
func (p ToolPrincipal) Equals(o Principal) bool { return principalEquals(p, o) }

// Equals reports whether other is the same principal. An anonymous principal equals nothing.
func (AnonymousPrincipal) Equals(Principal) bool { return false }

// EqualsRef reports whether this principal is the one ref names.
func (p UserPrincipal) EqualsRef(r PrincipalRef) bool { return p.Pair == r }

// EqualsRef reports whether this principal is the one ref names.
func (p AccountPrincipal) EqualsRef(r PrincipalRef) bool { return p.Pair == r }

// EqualsRef reports whether this principal is the one ref names.
func (p AgentPrincipal) EqualsRef(r PrincipalRef) bool { return p.Pair == r }

// EqualsRef reports whether this principal is the one ref names.
func (p ToolPrincipal) EqualsRef(r PrincipalRef) bool { return p.Pair == r }

// EqualsRef reports whether this principal is the one ref names.
func (AnonymousPrincipal) EqualsRef(PrincipalRef) bool { return false }

func principalEquals(p, other Principal) bool {
	mine, ok := p.Ref()
	if !ok {
		return false
	}
	theirs, ok := other.Ref()
	return ok && mine == theirs
}

func principalText(p Principal) string {
	ref, ok := p.Ref()
	if !ok {
		return "anonymous"
	}
	return string(p.Kind()) + " " + ref.String()
}

// String is "<kind> <ref text>", or "anonymous". It never includes claims.
func (p UserPrincipal) String() string { return principalText(p) }

// String is "<kind> <ref text>", or "anonymous". It never includes claims.
func (p AccountPrincipal) String() string { return principalText(p) }

// String is "<kind> <ref text>", or "anonymous". It never includes claims.
func (p AgentPrincipal) String() string { return principalText(p) }

// String is "<kind> <ref text>", or "anonymous". It never includes claims.
func (p ToolPrincipal) String() string { return principalText(p) }

// String is "<kind> <ref text>", or "anonymous". It never includes claims.
func (AnonymousPrincipal) String() string { return "anonymous" }

// TokenProvenance says which token a caller presented and who made it. The ID
// is the token's own, never its secret.
type TokenProvenance struct {
	ID    UUID
	Maker Option[PrincipalRef]
}

// Caller says who caused the running invocation: the Original principal that
// started the chain, the Immediate principal that made this call, and the
// provenance of the Tokens on the way.
type Caller struct {
	Original  Principal
	Immediate Principal
	Tokens    []TokenProvenance
}

// AnonymousCaller is a caller with no identity: anonymous in both roles and no tokens.
func AnonymousCaller() Caller {
	return Caller{Original: AnonymousPrincipal{}, Immediate: AnonymousPrincipal{}}
}

func refFromWit(r common.PrincipalRef) PrincipalRef {
	return PrincipalRef{Issuer: r.Issuer, Subject: r.Subject}
}

func principalFromWit(p common.Principal) Principal {
	switch p.Tag() {
	case common.PrincipalUser:
		u := p.User()
		return UserPrincipal{
			Pair:            refFromWit(u.Pair),
			Claims:          u.Claims,
			AuthenticatedAt: time.Unix(u.AuthenticatedAt.Seconds, int64(u.AuthenticatedAt.Nanoseconds)).UTC(),
		}
	case common.PrincipalAccount:
		a := p.Account()
		return AccountPrincipal{
			Pair:           refFromWit(a.Pair),
			AccountID:      uuidFromWit(a.AccountId.Uuid),
			Authentication: AccountAuthentication(a.Authentication),
		}
	case common.PrincipalAgent:
		a := p.Agent()
		return AgentPrincipal{
			Pair:          refFromWit(a.Pair),
			AgentID:       agentIDFromWit(a.AgentId),
			AgentType:     a.AgentType,
			EnvironmentID: uuidFromWit(a.EnvironmentId.Uuid),
		}
	case common.PrincipalTool:
		t := p.Tool()
		return ToolPrincipal{
			Pair:          refFromWit(t.Pair),
			EnvironmentID: uuidFromWit(t.EnvironmentId.Uuid),
			ToolName:      t.ToolName,
		}
	case common.PrincipalAnonymous:
		return AnonymousPrincipal{}
	default:
		// An unknown principal must never pass as anonymous.
		panic(fmt.Sprintf("golem: unhandled principal case %d", p.Tag()))
	}
}

func callerFromWit(c common.Caller) Caller {
	out := Caller{Original: principalFromWit(c.Original), Immediate: principalFromWit(c.Immediate)}
	for _, t := range c.Tokens {
		tp := TokenProvenance{ID: uuidFromWit(t.Id)}
		if t.Maker.IsSome() {
			tp.Maker = Some(refFromWit(t.Maker.Some()))
		} else {
			tp.Maker = None[PrincipalRef]()
		}
		out.Tokens = append(out.Tokens, tp)
	}
	return out
}

var (
	principalType       = reflect.TypeFor[Principal]()
	callerType          = reflect.TypeFor[Caller]()
	tokenProvenanceType = reflect.TypeFor[TokenProvenance]()
)

// isLiveValueType reports whether t is a [Principal], a [Caller], a
// [TokenProvenance] or one of the concrete principal kinds: values the host
// tells an invocation about, which can never be method data.
func isLiveValueType(t reflect.Type) bool {
	return t == principalType || t == callerType || t == tokenProvenanceType ||
		(t.Kind() != reflect.Interface && t.Implements(principalType))
}

// errLiveValue is the refusal of an ordinary serialization of a live value.
var errLiveValue = errors.New("a Caller, a Principal and a TokenProvenance are live values and cannot be serialized; store ctx.Caller().Original.Ref() instead")

// MarshalJSON refuses: a caller is a live value. Store ctx.Caller().Original.Ref() instead.
func (Caller) MarshalJSON() ([]byte, error) { return nil, errLiveValue }

// MarshalJSON refuses: a token provenance is a live value.
func (TokenProvenance) MarshalJSON() ([]byte, error) { return nil, errLiveValue }

// MarshalJSON refuses: a principal is a live value. Store its Ref() instead.
func (UserPrincipal) MarshalJSON() ([]byte, error) { return nil, errLiveValue }

// MarshalJSON refuses: a principal is a live value. Store its Ref() instead.
func (AccountPrincipal) MarshalJSON() ([]byte, error) { return nil, errLiveValue }

// MarshalJSON refuses: a principal is a live value. Store its Ref() instead.
func (AgentPrincipal) MarshalJSON() ([]byte, error) { return nil, errLiveValue }

// MarshalJSON refuses: a principal is a live value. Store its Ref() instead.
func (ToolPrincipal) MarshalJSON() ([]byte, error) { return nil, errLiveValue }

// MarshalJSON refuses: a principal is a live value.
func (AnonymousPrincipal) MarshalJSON() ([]byte, error) { return nil, errLiveValue }

// misplacedCaller is the message of every build-time refusal of a live value.
const misplacedCaller = "Principal and Caller cannot appear in a method, constructor or tool signature, or inside a value type. Read the caller with ctx.Caller() (golem.Caller), and store ctx.Caller().Original.Ref() as a PrincipalRef."

// lazyCaller returns a function that asks the host for the caller on its first
// call and remembers the answer, and a function that ends the call. Reading is
// deferred because it allocates, and an allocation can start a garbage
// collection whose clock reads record in the oplog: an invocation that never
// looks at its caller never pays for it. Once the call has ended the host holds
// another invocation's caller, so a first read after that panics, and a caller
// read during the call stays what it was. A call that returned a stream ends
// with returnedStream set: its producer runs after the return, so the end reads
// the caller while the call still holds it, and the producer gets that value.
func lazyCaller() (read func() Caller, end func(returnedStream bool)) {
	var c Caller
	var have, ended bool
	read = func() Caller {
		if !have {
			if ended {
				panic(callerEnded)
			}
			c, have = callerFromWit(currentCaller()), true
		}
		return c
	}
	return read, func(returnedStream bool) {
		if returnedStream && !have && !ended {
			c, have = callerFromWit(currentCaller()), true
		}
		ended = true
	}
}

// callerEnded is the panic of a first read of the caller after the call ended.
const callerEnded = "this context belongs to a call that has ended; store ctx.Caller().Original.Ref() instead"

// callerOf reads a lazy caller, which is anonymous where there is no invocation.
func callerOf(read func() Caller) Caller {
	if read == nil {
		return AnonymousCaller()
	}
	return read()
}
