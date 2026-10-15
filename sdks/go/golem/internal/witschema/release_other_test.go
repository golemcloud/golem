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

package witschema

import (
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	"testing"
)

func TestReleaseRawTreeIncludesUnreachableCapabilities(t *testing.T) {
	original := releaseNode
	defer func() { releaseNode = original }()
	var tags []uint8
	releaseNode = func(n types.SchemaValueNode) { tags = append(tags, n.Tag()) }
	ReleaseTree(types.SchemaValueTree{Root: 100, ValueNodes: []types.SchemaValueNode{
		types.MakeSchemaValueNodeStringValue("plain"),
		types.MakeSchemaValueNodeSecretValue(nil),
		types.MakeSchemaValueNodeQuotaTokenHandle(nil),
		types.MakeSchemaValueNodePermissionCardHandle(nil),
		types.MakeSchemaValueNodeStreamValue(nil),
	}})
	want := []uint8{types.SchemaValueNodeSecretValue, types.SchemaValueNodeQuotaTokenHandle, types.SchemaValueNodePermissionCardHandle, types.SchemaValueNodeStreamValue}
	if len(tags) != len(want) {
		t.Fatalf("released %v", tags)
	}
	for i := range want {
		if tags[i] != want[i] {
			t.Fatalf("released %v", tags)
		}
	}
}
