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

//go:build !wasip1

package golem

import (
	"errors"

	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
)

// Off the wasm target there is no host to read or reveal a secret; see
// secret_wasm.go.

var errSecretOutsideComponent = errors.New("golem: secrets are only available inside a component")

func (s *Secret[T]) secretBindPath([]string) {
	s.read = func() (T, error) {
		var zero T
		return zero, errSecretOutsideComponent
	}
	s.take = func() (*types.Secret, error) { return nil, errSecretOutsideComponent }
}

func (s *Secret[T]) secretAdopt(*types.Secret) {
	s.read = func() (T, error) {
		var zero T
		return zero, errSecretOutsideComponent
	}
	s.take = func() (*types.Secret, error) { return nil, errSecretOutsideComponent }
}
