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

package tool

import (
	"fmt"
	"github.com/golemcloud/golem/sdks/go/golem"
	"github.com/golemcloud/golem/sdks/go/golem/internal/engine"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	"reflect"
	"regexp"
)

// --- tool argument setters ------------------------------------------------------

// restrictable gives a tool argument binding its restriction setters. A
// returns the binding, for chaining; T is the argument's value type, which
// typed bounds are spelled in.
type restrictable[A any, T any] struct {
	rb   *argBinding
	self A
}

func (r restrictable[A, T]) restriction() *engine.Restriction {
	if r.rb.restrict == nil {
		r.rb.restrict = &engine.Restriction{}
	}
	return r.rb.restrict
}

func (r restrictable[A, T]) bound(dst **string, v T) {
	text, err := engine.BoundText(reflect.ValueOf(&v).Elem())
	if err != nil {
		r.rb.restrictErr = err
		return
	}
	*dst = &text
}

// Range bounds a number or quantity argument, inclusively.
func (r restrictable[A, T]) Range(min, max T) A {
	r.bound(&r.restriction().Min, min)
	r.bound(&r.restriction().Max, max)
	return r.self
}

// MinValue is the smallest value a number or quantity argument accepts.
func (r restrictable[A, T]) MinValue(min T) A {
	r.bound(&r.restriction().Min, min)
	return r.self
}

// MaxValue is the largest value a number or quantity argument accepts.
func (r restrictable[A, T]) MaxValue(max T) A {
	r.bound(&r.restriction().Max, max)
	return r.self
}

// Unit names the unit a number argument is measured in.
func (r restrictable[A, T]) Unit(unit string) A {
	r.restriction().Unit = &unit
	return r.self
}

// Languages lists the BCP-47 languages a text argument may be in.
func (r restrictable[A, T]) Languages(codes ...string) A {
	r.restriction().Languages = &codes
	return r.self
}

// MinLength is the fewest characters a text argument may have.
func (r restrictable[A, T]) MinLength(n uint32) A {
	r.restriction().MinLength = &n
	return r.self
}

// MaxLength is the most characters a text argument may have.
func (r restrictable[A, T]) MaxLength(n uint32) A {
	r.restriction().MaxLength = &n
	return r.self
}

// Regex is a pattern a text argument must match.
func (r restrictable[A, T]) Regex(pattern string) A {
	if _, err := regexp.Compile(pattern); err != nil {
		r.rb.restrictErr = fmt.Errorf("regex %q: %w", pattern, err)
	}
	r.restriction().Regex = &pattern
	return r.self
}

// Mime lists the media types a binary or path argument may have.
func (r restrictable[A, T]) Mime(types ...string) A {
	r.restriction().Mime = &types
	return r.self
}

// MinBytes is the smallest a binary argument may be.
func (r restrictable[A, T]) MinBytes(n uint32) A {
	r.restriction().MinBytes = &n
	return r.self
}

// MaxBytes is the largest a binary argument may be.
func (r restrictable[A, T]) MaxBytes(n uint32) A {
	r.restriction().MaxBytes = &n
	return r.self
}

// Direction is which way a path argument's file flows.
func (r restrictable[A, T]) Direction(d golem.PathDirection) A {
	w := types.PathDirection(d)
	r.restriction().Direction = &w
	return r.self
}

// PathKind is whether a path argument names a file, a directory or either.
func (r restrictable[A, T]) PathKind(k golem.PathKind) A {
	w := types.PathKind(k)
	r.restriction().Kind = &w
	return r.self
}

// Extensions lists the file extensions a path argument may have.
func (r restrictable[A, T]) Extensions(exts ...string) A {
	r.restriction().Extensions = &exts
	return r.self
}

// Schemes lists the schemes a URL argument may use.
func (r restrictable[A, T]) Schemes(schemes ...string) A {
	r.restriction().Schemes = &schemes
	return r.self
}

// Hosts lists the hosts a URL argument may name.
func (r restrictable[A, T]) Hosts(hosts ...string) A {
	r.restriction().Hosts = &hosts
	return r.self
}
