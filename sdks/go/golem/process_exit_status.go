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
	"fmt"
	"reflect"

	"github.com/golemcloud/golem/sdks/go/golem/internal/engine"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
)

// ProcessExitStatus is the status of a completed command, as a shell sees it.
// A tool command that returns it, such as a Command[Args, golem.ProcessExitStatus],
// reports that status, which is not a failure to run the command: the built-in
// Bash tool takes ExitCode as the command's `$?`.
//
// Every SDK publishes it as the same named record,
// golem.tool.ProcessExitStatus { exit_code: u8 }.
type ProcessExitStatus struct {
	ExitCode uint8
}

const processExitStatusID = "golem.tool.ProcessExitStatus"

// compileProcessExitStatus spells the shared record: its field is exit_code,
// which no Go field name derives, and it is always a named definition, so
// another language finds it by its type-id.
func compileProcessExitStatus(c *engine.Codec, code *engine.Codec) {
	c.Named = true
	c.Body = func(g *engine.GraphBuilder) types.SchemaTypeBody {
		return types.MakeSchemaTypeBodyRecordType([]types.NamedFieldType{{Name: "exit_code", Body: g.Node(code)}})
	}
	c.Encode = func(b *engine.ValBuilder, v reflect.Value) int32 {
		return b.Push(types.MakeSchemaValueNodeRecordValue([]int32{code.Encode(b, v.Field(0))}))
	}
	c.Decode = func(d *engine.Decoder, dst reflect.Value, idx int32) error {
		n, err := d.Node(idx)
		if err != nil {
			return err
		}
		if n.Tag() != types.SchemaValueNodeRecordValue || len(n.RecordValue()) != 1 {
			return fmt.Errorf("cannot decode value node (tag %d) into %s", n.Tag(), c.Typ)
		}
		return code.Decode(d, dst.Field(0), n.RecordValue()[0])
	}
}
