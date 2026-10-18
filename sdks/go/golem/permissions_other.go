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
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	perm "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_permissions_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// Off the wasm target there is no host: binding the generated calls here would
// drag their //go:wasmimport declarations into a native link that has no
// bodies for them. Every operation panics; tests install a fake.
var permissionsHostOps = permissionsHost{
	id:        func(*types.PermissionCard) types.CardId { panic(ErrPermissionsOutsideComponent) },
	parents:   func(*types.PermissionCard) []types.CardId { panic(ErrPermissionsOutsideComponent) },
	expiresAt: func(*types.PermissionCard) witTypes.Option[perm.Timestamp] { panic(ErrPermissionsOutsideComponent) },
	inspect: func(*types.PermissionCard) witTypes.Result[perm.CardView, perm.PermissionError] {
		panic(ErrPermissionsOutsideComponent)
	},
	derive: func(*types.PermissionCard, []perm.PatternGrant, []perm.PatternGrant, []perm.PatternGrant, []perm.PatternGrant, witTypes.Option[perm.Duration]) witTypes.Result[perm.PersistentCard, perm.PermissionError] {
		panic(ErrPermissionsOutsideComponent)
	},
	deriveFromWallet: func([]perm.PatternGrant, []perm.PatternGrant, []perm.PatternGrant, []perm.PatternGrant, witTypes.Option[perm.Duration]) witTypes.Result[perm.PersistentCard, perm.PermissionError] {
		panic(ErrPermissionsOutsideComponent)
	},
	deriveScope: func([]*types.PermissionCard, []perm.PatternGrant, []perm.PatternGrant, []perm.PatternGrant, []perm.PatternGrant) witTypes.Result[*types.PermissionCard, perm.PermissionError] {
		panic(ErrPermissionsOutsideComponent)
	},
	revoke: func(*types.PermissionCard) witTypes.Result[uint32, perm.PermissionError] {
		panic(ErrPermissionsOutsideComponent)
	},
	install: func(*types.PermissionCard, perm.Holder) witTypes.Result[witTypes.Unit, perm.PermissionError] {
		panic(ErrPermissionsOutsideComponent)
	},
	wallet:        func() []perm.WalletCard { panic(ErrPermissionsOutsideComponent) },
	walletVersion: func() perm.WalletVersionToken { panic(ErrPermissionsOutsideComponent) },
	listModules:   func() []KernelModule { panic(ErrPermissionsOutsideComponent) },
	validateGrant: func(perm.PatternGrant) witTypes.Result[witTypes.Unit, perm.PermissionError] {
		panic(ErrPermissionsOutsideComponent)
	},
	release: func(*types.PermissionCard) { panic(ErrPermissionsOutsideComponent) },
}
