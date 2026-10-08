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

//go:build wasip1

package golem

import (
	"fmt"
	"github.com/golemcloud/golem/sdks/go/golem/internal/engine"
	"reflect"

	"github.com/golemcloud/golem/sdks/go/golem/internal/secretref"
	host "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_host"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	reveal "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_secrets_reveal"
)

// The host side of secrets: reading and revealing them, and minting handles to
// send or lend. Native builds have stubs (secret_other.go).

// secretBorrower lends a secret's handle to a host call without moving it.
type secretBorrower interface {
	secretBorrow() (*types.Secret, func(), error)
}

func (s Secret[T]) secretBorrow() (*types.Secret, func(), error) {
	if s.borrow == nil {
		return nil, nil, fmt.Errorf("golem: Secret has no source; obtain it from the agent's config")
	}
	return s.borrow()
}

func init() {
	secretref.Borrow = func(secret any) (*types.Secret, func(), error) {
		b, ok := secret.(secretBorrower)
		if !ok {
			return nil, nil, fmt.Errorf("golem: %T is not a golem.Secret", secret)
		}
		return b.secretBorrow()
	}
}

func (s *Secret[T]) secretBindPath(path []string) {
	s.read = func() (T, error) { return readSecretValue[T](defs, path) }
	s.take = func() (*types.Secret, error) { return configSecretHandle[T](defs, path) }
	s.borrow = func() (*types.Secret, func(), error) {
		h, err := configSecretHandle[T](defs, path)
		if err != nil {
			return nil, nil, err
		}
		return h, h.Drop, nil
	}
}

// secretAdopt initialises a zero Secret with a received handle, which Get
// reveals and sending moves.
func (s *Secret[T]) secretAdopt(h *types.Secret) {
	held := &h
	s.read = func() (T, error) {
		if *held == nil {
			var zero T
			return zero, ErrSecretMoved
		}
		return revealSecret[T](defs, nil, *held)
	}
	s.borrow = func() (*types.Secret, func(), error) {
		if *held == nil {
			return nil, nil, ErrSecretMoved
		}
		return *held, func() {}, nil
	}
	s.take = func() (*types.Secret, error) {
		if *held == nil {
			return nil, ErrSecretMoved
		}
		h := *held
		*held = nil
		return h, nil
	}
}

// readSecretValue fetches, reveals, and decodes a config secret's CURRENT value.
// [Secret.Get] calls it on every access, so each read returns the latest value
// (a fresh get-config-value mints a handle pinned to the current revision, which
// reveal then unpacks). Host-backed — reachable only from Secret.Get, never from
// the config-materialization path.
func readSecretValue[T any](d *definitions, path []string) (T, error) {
	var zero T
	handleRes := host.GetConfigValue(path, d.GraphForType(reflect.TypeFor[Secret[T]]()))
	if handleRes.IsErr() {
		return zero, configValueErrorToGo(path, handleRes.Err())
	}
	handle, err := extractSecretHandle(path, handleRes.Ok())
	if err != nil {
		return zero, err
	}
	return revealSecret[T](d, path, handle)
}

// configSecretHandle mints a fresh handle to a config secret, for sending it.
func configSecretHandle[T any](d *definitions, path []string) (*types.Secret, error) {
	handleRes := host.GetConfigValue(path, d.GraphForType(reflect.TypeFor[Secret[T]]()))
	if handleRes.IsErr() {
		return nil, configValueErrorToGo(path, handleRes.Err())
	}
	return extractSecretHandle(path, handleRes.Ok())
}

// revealSecret reads a secret handle's plaintext and decodes it as T; path
// names a config secret in errors, nil for a received one.
func revealSecret[T any](d *definitions, path []string, handle *types.Secret) (T, error) {
	var zero T
	innerType := reflect.TypeFor[T]()
	res := reveal.Reveal(handle, d.GraphForType(innerType))
	if res.IsErr() {
		return zero, secretErrorToGo(path, res.Err())
	}
	dst := reflect.New(innerType).Elem()
	dec := engine.Decoder{Nodes: res.Ok().ValueNodes}
	if err := d.Compile(innerType).Decode(&dec, dst, res.Ok().Root); err != nil {
		return zero, fmt.Errorf("golem/secret %v: %w", path, err)
	}
	return dst.Interface().(T), nil
}
