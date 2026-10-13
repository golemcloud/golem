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

// Package secretref lets SDK subpackages lend a golem.Secret's host handle to
// another host call without moving it. The golem package installs Borrow.
package secretref

import types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"

// Borrow returns a handle for secret, which must be a golem.Secret, and a
// release to call once the host call that borrows it has returned.
var Borrow func(secret any) (handle *types.Secret, release func(), err error)
