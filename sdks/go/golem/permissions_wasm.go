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
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	derive "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_permissions_derive"
	inspect "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_permissions_inspect"
	kernel "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_permissions_kernel_introspection"
	revoke "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_permissions_revoke"
	perm "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_permissions_types"
	wallet "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_permissions_wallet"
)

var permissionsHostOps = permissionsHost{
	id:               perm.Id,
	parents:          perm.Parents,
	expiresAt:        perm.ExpiresAt,
	inspect:          inspect.InspectCard,
	derive:           derive.Derive,
	deriveFromWallet: derive.DeriveFromWallet,
	deriveScope:      derive.DeriveScope,
	revoke:           revoke.RevokeCard,
	install:          wallet.InstallCard,
	wallet:           wallet.SelfWallet,
	walletVersion:    wallet.SelfVersion,
	listModules: func() []KernelModule {
		ms := kernel.ListModules()
		out := make([]KernelModule, len(ms))
		for i, m := range ms {
			out[i] = KernelModule{ClassName: m.ClassName, Verbs: m.Verbs}
		}
		return out
	},
	validateGrant: kernel.ValidateGrant,
	release:       func(c *types.PermissionCard) { c.Drop() },
}
