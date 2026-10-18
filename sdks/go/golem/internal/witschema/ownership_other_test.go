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

package witschema_test

import (
	"errors"
	"reflect"
	"testing"

	"github.com/golemcloud/golem/sdks/go/golem"
	"github.com/golemcloud/golem/sdks/go/golem/internal/engine"
	"github.com/golemcloud/golem/sdks/go/golem/internal/link"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	"github.com/golemcloud/golem/sdks/go/golem/internal/witschema"
)

func TestDecodeOwnedReleasesRejectedCapabilities(t *testing.T) {
	type result struct {
		First golem.Secret[string]
		Later string
	}
	for _, outcome := range []string{"malformed", "unexpected", "panic", "success"} {
		t.Run(outcome, func(t *testing.T) {
			released := witschema.RecordReleasedNodes(t)
			tree := types.SchemaValueTree{Root: 0, ValueNodes: []types.SchemaValueNode{
				types.MakeSchemaValueNodeRecordValue([]int32{1, 2}),
				types.MakeSchemaValueNodeSecretValue(new(types.Secret)),
				types.MakeSchemaValueNodeStringValue("valid"),
			}}
			if outcome == "malformed" {
				tree.ValueNodes[2] = types.MakeSchemaValueNodeBoolValue(true)
				// These later capabilities are never reached by the decoder.
				tree.ValueNodes = append(tree.ValueNodes,
					types.MakeSchemaValueNodeQuotaTokenHandle(new(types.QuotaToken)),
					types.MakeSchemaValueNodePermissionCardHandle(new(types.PermissionCard)),
					types.MakeSchemaValueNodeStreamValue(new(types.SchemaValueStream)))
			}
			codec := link.NewEngine().Compile(reflect.TypeFor[result]())
			rejected := errors.New("command declares no result")
			var delivered result
			var err error
			var panicked any
			func() {
				defer func() { panicked = recover() }()
				delivered, err = witschema.DecodeOwned(tree, func() (result, error) {
					var out result
					if outcome == "unexpected" {
						return out, rejected
					}
					dec := engine.Decoder{Nodes: tree.ValueNodes}
					if err := codec.Decode(&dec, reflect.ValueOf(&out).Elem(), tree.Root); err != nil {
						return out, err
					}
					if outcome == "panic" {
						panic(rejected)
					}
					return out, nil
				})
			}()
			switch outcome {
			case "malformed":
				if err == nil || panicked != nil {
					t.Fatalf("malformed decode: %v, panic %v", err, panicked)
				}
			case "unexpected":
				if !errors.Is(err, rejected) || panicked != nil {
					t.Fatalf("unexpected payload: %v, panic %v", err, panicked)
				}
			case "panic":
				if panicked != rejected {
					t.Fatalf("panic replaced: %v", panicked)
				}
			case "success":
				if err != nil || panicked != nil || len(*released) != 0 || delivered.Later != "valid" {
					t.Fatalf("delivered capabilities released: %v, %v, %v", *released, err, panicked)
				}
				return
			}
			want := map[uint8]int{types.SchemaValueNodeSecretValue: 1}
			if outcome == "malformed" {
				want[types.SchemaValueNodeQuotaTokenHandle] = 1
				want[types.SchemaValueNodePermissionCardHandle] = 1
				want[types.SchemaValueNodeStreamValue] = 1
			}
			counts := map[uint8]int{}
			for _, node := range *released {
				counts[node.Tag()]++
			}
			if !reflect.DeepEqual(counts, want) {
				t.Fatalf("released %v, want each capability exactly once: %v", counts, want)
			}
		})
	}
}
