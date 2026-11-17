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

package reflection

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"strings"
	"sync"

	core "github.com/golemcloud/golem/sdks/go/core/schema"
	"github.com/golemcloud/golem/sdks/go/golem/tool"
)

// processExitStatusID is the type-id of the shared record a command returns
// to report a process exit status.
const processExitStatusID = "golem.tool.ProcessExitStatus"

// RunCommandLine runs a command line against the tool the way a process runs,
// with the rules the bash tool and the JavaScript process adapter use:
//
//   - help text goes to stdout, with status 0;
//   - a command line that does not parse is reported on stderr, with status 2;
//   - stdin goes to the command only when it declares standard input;
//   - the command's outputs are copied to stdout and stderr while it runs;
//   - a ProcessExitStatus result gives the status; another result goes to
//     stdout, a string as it is and other values as JSON, unless the command
//     writes its own standard output;
//   - a failed call is reported on stderr: denied gives 3, an unknown tool 127,
//     cancellation 130, invalid input 2, a declared error the status it
//     declares (1 when it declares none), anything else 1.
//
// variables are as for [Tool.ParseArgsWithVariables].
func (c *ToolClient) RunCommandLine(args []string, variables map[string]string,
	stdin io.Reader, stdout, stderr io.Writer) uint8 {
	parsed, err := c.tool.ParseArgsWithVariables(args, variables)
	if err != nil {
		fmt.Fprintln(stderr, commandLineError(err.Error()))
		return 2
	}
	if parsed.Help != "" {
		_, _ = io.WriteString(stdout, parsed.Help)
		return 0
	}
	cmd, _ := c.tool.Command(parsed.Path)
	body := cmd.node().Body.Some()
	out, declared := cmd.Output().Get()
	resultIsStatus := declared && out.TypeName() == processExitStatusID
	if resultIsStatus && !isProcessExitStatus(out) {
		fmt.Fprintln(stderr, "ProcessExitStatus must define exactly one exit_code: u8 field")
		return 2
	}
	if body.Stdin.IsNone() {
		stdin = nil
	}
	inv, err := c.Start(parsed.Path, parsed.Input, stdin)
	if err != nil {
		return callFailure(cmd, err, stderr)
	}
	var wg sync.WaitGroup
	var relayMu sync.Mutex
	var relayErr error
	relay := func(r io.ReadCloser, w io.Writer) {
		if r == nil {
			return
		}
		wg.Add(1)
		go func() {
			defer wg.Done()
			defer r.Close()
			if _, err := io.Copy(w, r); err != nil {
				relayMu.Lock()
				relayErr = errors.Join(relayErr, err)
				relayMu.Unlock()
			}
		}()
	}
	relay(inv.Stdout(), stdout)
	relay(inv.Stderr(), stderr)
	result, err := inv.Wait()
	wg.Wait()
	if err != nil {
		return callFailure(cmd, err, stderr)
	}
	// A failed output decides the status before the result does.
	if relayErr != nil {
		fmt.Fprintln(stderr, relayErr)
		return 1
	}
	if resultIsStatus {
		code, err := exitCode(result)
		if err != nil {
			fmt.Fprintln(stderr, err)
			return 1
		}
		return code
	}
	if body.Stdout.IsSome() || result == nil {
		return 0
	}
	if text, ok := result.(string); ok {
		_, _ = io.WriteString(stdout, text)
		return 0
	}
	var rendered bytes.Buffer
	enc := json.NewEncoder(&rendered)
	enc.SetEscapeHTML(false)
	if err := enc.Encode(result); err != nil {
		fmt.Fprintf(stderr, "result: %v\n", err)
		return 1
	}
	_, _ = stdout.Write(rendered.Bytes())
	return 0
}

// commandLineError describes a command line that does not parse. The parser
// cannot build opaque capabilities, such as secrets, from text.
func commandLineError(message string) string {
	if strings.Contains(message, "capabilit") || strings.Contains(message, "secret") {
		return "secret and other capability arguments are unsupported: " + message
	}
	return message
}

// isProcessExitStatus reports whether the reserved ProcessExitStatus type-id
// names exactly one exit_code: u8 field without restrictions, the contract
// golem_schema::tool::process::process_status_type checks.
func isProcessExitStatus(out core.Ref) bool {
	def, found := out.Graph().Def(processExitStatusID)
	if !found {
		return false
	}
	record, ok := def.Body.Body.(core.RecordType)
	if !ok || len(record.Fields) != 1 || record.Fields[0].Name != "exit_code" {
		return false
	}
	code, ok := record.Fields[0].Body.Body.(core.U8Type)
	return ok && code.Restrictions == nil
}

func exitCode(result any) (uint8, error) {
	fields, ok := result.(map[string]any)
	if !ok {
		return 0, errors.New("result: expected ProcessExitStatus record")
	}
	code, ok := fields["exit_code"].(uint64)
	if !ok {
		return 0, errors.New("result: expected exactly one u8 exit status value")
	}
	return uint8(code), nil
}

func callFailure(cmd Command, err error, stderr io.Writer) uint8 {
	var ce *tool.CallError
	if !errors.As(err, &ce) {
		fmt.Fprintln(stderr, err)
		return 1
	}
	message := ce.Message
	if message == "" {
		message = err.Error()
	}
	var code uint8 = 1
	switch ce.Kind {
	case tool.CallDenied:
		code = 3
	case tool.CallUnknownTool:
		code = 127
	case tool.CallCancelled:
		code, message = 130, "Cancelled"
	case tool.CallInvalidInput, tool.CallConstraintViolation:
		code = 2
	case tool.CallDeclaredError:
		for _, e := range cmd.Errors() {
			if e.Name == ce.ErrorName {
				code = e.ExitCode
			}
		}
		message = "tool error: " + ce.ErrorName
		if payload, err := ce.Payload().JSON(); err == nil {
			if data, err := json.Marshal(payload); err == nil {
				message += ": " + string(data)
			}
		}
	}
	fmt.Fprintln(stderr, message)
	return code
}
