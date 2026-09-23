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
	"reflect"

	toolCommon "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_tool_common"
)

// Command constraints.
//
// Some rules about which arguments may appear together cannot be expressed by
// the argument list alone. A constraint states one, referring to arguments by
// name:
//
//	golem.Constrain(
//	    golem.Mutex(golem.Present("json"), golem.Present("yaml")),
//	    golem.Implies(golem.AllOf(golem.Present("sign")), golem.AnyOf(golem.Present("key"))),
//	)
//
// The names are the wire names — field names with the first letter lower-cased.
// Every reference is resolved against the command's own arguments when the tool
// is discovered, so a typo is a definition error rather than a rule that
// silently never fires.

// Ref refers to an argument from inside a constraint. Build one with [Present]
// or [ValueIs].
type Ref struct {
	name string
	// value is the literal a value-is reference compares against, invalid for a
	// presence reference.
	value reflect.Value
}

// Present refers to the argument being supplied at all.
func Present(name string) Ref { return Ref{name: name} }

// ValueIs refers to the argument being supplied with a particular value. The
// literal's type must match the argument's declared type.
func ValueIs[T any](name string, value T) Ref {
	return Ref{name: name, value: reflect.ValueOf(&value).Elem()}
}

// RefSet is a quantified group of references, built with [AllOf] or [AnyOf].
type RefSet struct {
	quant toolCommon.Quantifier
	refs  []Ref
}

// AllOf holds when every reference in it holds.
func AllOf(refs ...Ref) RefSet { return RefSet{quant: toolCommon.QuantifierAll, refs: refs} }

// AnyOf holds when at least one reference in it holds.
func AnyOf(refs ...Ref) RefSet { return RefSet{quant: toolCommon.QuantifierAny, refs: refs} }

// constraintKind selects which rule a [Constraint] expresses.
type constraintKind uint8

const (
	constraintRequiresAll constraintKind = iota
	constraintAllOrNone
	constraintRequiresAny
	constraintMutexGroups
	constraintImplies
	constraintForbids
)

// Constraint is one rule about a command's arguments.
type Constraint struct {
	kind   constraintKind
	refs   []Ref
	groups [][]Ref
	lhs    RefSet
	rhs    RefSet
}

// RequiresAll holds only when every reference holds.
func RequiresAll(refs ...Ref) Constraint {
	return Constraint{kind: constraintRequiresAll, refs: refs}
}

// AllOrNone holds when either every reference holds or none does.
func AllOrNone(refs ...Ref) Constraint {
	return Constraint{kind: constraintAllOrNone, refs: refs}
}

// RequiresAny holds when at least one reference holds.
func RequiresAny(refs ...Ref) Constraint {
	return Constraint{kind: constraintRequiresAny, refs: refs}
}

// Mutex allows at most one of the references to hold.
func Mutex(refs ...Ref) Constraint {
	return Constraint{kind: constraintMutexGroups, groups: [][]Ref{refs}}
}

// MutexGroups states several independent mutual exclusions at once.
func MutexGroups(groups ...[]Ref) Constraint {
	return Constraint{kind: constraintMutexGroups, groups: groups}
}

// Implies requires rhs to hold whenever lhs does.
func Implies(lhs, rhs RefSet) Constraint {
	return Constraint{kind: constraintImplies, lhs: lhs, rhs: rhs}
}

// Forbids rejects rhs holding whenever lhs does.
func Forbids(lhs RefSet, rhs ...Ref) Constraint {
	return Constraint{kind: constraintForbids, lhs: lhs, rhs: RefSet{refs: rhs}}
}

// Constrain attaches constraints to a command.
func Constrain(cs ...Constraint) CommandOpt {
	return func(o *commandOpts) { o.constraints = append(o.constraints, cs...) }
}

// buildConstraints lowers a command's constraints, resolving every reference
// against the command's own arguments.
func (d *definitions) buildConstraints(ce *commandEntry, fields []toolArgField) []toolCommon.Constraint {
	byName := make(map[string]toolArgField, len(fields))
	for _, f := range fields {
		byName[f.name] = f
	}

	ref := func(r Ref) toolCommon.Ref {
		f, known := byName[r.name]
		if !known {
			d.recordErr("", "", "command %s constrains %q, which it does not declare",
				commandLabel(ce.path), r.name)
			return toolCommon.MakeRefPresent(r.name)
		}
		if !r.value.IsValid() {
			return toolCommon.MakeRefPresent(r.name)
		}
		if r.value.Type() != f.elem {
			d.recordErr("", "", "command %s compares %q against a %s, but it is declared as %s",
				commandLabel(ce.path), r.name, r.value.Type(), f.elem)
			return toolCommon.MakeRefPresent(r.name)
		}
		return toolCommon.MakeRefValueIs(toolCommon.ValueIsRef{
			Name:  r.name,
			Value: encodeWith(f.codec, r.value),
		})
	}

	refs := func(in []Ref) []toolCommon.Ref {
		out := make([]toolCommon.Ref, 0, len(in))
		for _, r := range in {
			out = append(out, ref(r))
		}
		return out
	}

	out := make([]toolCommon.Constraint, 0, len(ce.opts.constraints))
	for _, c := range ce.opts.constraints {
		switch c.kind {
		case constraintRequiresAll:
			out = append(out, toolCommon.MakeConstraintRequiresAll(refs(c.refs)))
		case constraintAllOrNone:
			out = append(out, toolCommon.MakeConstraintAllOrNone(refs(c.refs)))
		case constraintRequiresAny:
			out = append(out, toolCommon.MakeConstraintRequiresAny(refs(c.refs)))
		case constraintMutexGroups:
			groups := make([]toolCommon.RefGroup, 0, len(c.groups))
			for _, g := range c.groups {
				groups = append(groups, toolCommon.RefGroup{Refs: refs(g)})
			}
			out = append(out, toolCommon.MakeConstraintMutexGroups(groups))
		case constraintImplies:
			out = append(out, toolCommon.MakeConstraintImplies(toolCommon.ImpliesC{
				LhsQuant: c.lhs.quant, Lhs: refs(c.lhs.refs),
				RhsQuant: c.rhs.quant, Rhs: refs(c.rhs.refs),
			}))
		case constraintForbids:
			out = append(out, toolCommon.MakeConstraintForbids(toolCommon.ForbidsC{
				LhsQuant: c.lhs.quant, Lhs: refs(c.lhs.refs),
				Rhs: refs(c.rhs.refs),
			}))
		}
	}
	return out
}
