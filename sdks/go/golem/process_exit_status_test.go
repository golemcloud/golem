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
	"testing"

	"github.com/golemcloud/golem/sdks/go/golem/internal/engine"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
)

// The status is the record every SDK shares: a reference to the named
// definition golem.tool.ProcessExitStatus, whose one field is exit_code: u8.
func TestProcessExitStatusIsTheSharedNamedRecord(t *testing.T) {
	g := engine.GraphBuilder{E: defs.Engine}
	root := g.Node(defs.Compile(reflect.TypeFor[ProcessExitStatus]()))
	graph := g.Build()

	if graph.TypeNodes[root].Body.Tag() != types.SchemaTypeBodyRefType {
		t.Fatal("the status must publish as a ref-type")
	}
	if len(graph.Defs) != 1 || graph.Defs[0].Id != "golem.tool.ProcessExitStatus" {
		t.Fatalf("defs %+v", graph.Defs)
	}
	body := graph.TypeNodes[graph.Defs[0].Body].Body
	if body.Tag() != types.SchemaTypeBodyRecordType {
		t.Fatalf("the definition body has tag %d, want a record", body.Tag())
	}
	fields := body.RecordType()
	if len(fields) != 1 || fields[0].Name != "exit_code" {
		t.Fatalf("fields %+v", fields)
	}
	code := graph.TypeNodes[fields[0].Body].Body
	if code.Tag() != types.SchemaTypeBodyU8Type || code.U8Type().IsSome() {
		t.Fatalf("exit_code must be an unrestricted u8, got tag %d", code.Tag())
	}
}

func TestProcessExitStatusRoundTrips(t *testing.T) {
	if got := roundTrip(t, ProcessExitStatus{ExitCode: 3}); got.ExitCode != 3 {
		t.Fatalf("round trip gave %+v", got)
	}
	type report struct {
		Status ProcessExitStatus
		Again  []ProcessExitStatus
	}
	in := report{Status: ProcessExitStatus{ExitCode: 255}, Again: []ProcessExitStatus{{}, {ExitCode: 1}}}
	if got := roundTrip(t, in); !reflect.DeepEqual(got, in) {
		t.Fatalf("round trip gave %+v", got)
	}
}
