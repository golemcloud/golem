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

import "errors"

// Off the wasm target there is no quota host to call; see quota_wasm.go.

var errQuotaOutsideComponent = errors.New("golem: quota tokens are only available inside a component")

func NewQuotaToken(string, uint64) QuotaToken { panic(errQuotaOutsideComponent) }

func (t QuotaToken) Reserve(uint64) (*Reservation, error) { return nil, errQuotaOutsideComponent }

func (t QuotaToken) Split(uint64) QuotaToken { panic(errQuotaOutsideComponent) }

func (t QuotaToken) Merge(QuotaToken) { panic(errQuotaOutsideComponent) }
