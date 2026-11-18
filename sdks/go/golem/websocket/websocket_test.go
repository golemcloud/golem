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

package websocket

import (
	"testing"
	"time"

	client "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_websocket_client"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// TestMessageRoundTrip — text/binary frames survive the toWit/fromWit conversion.
func TestMessageRoundTrip(t *testing.T) {
	txt := TextMessage("hello")
	if !txt.IsText() || txt.IsBinary() {
		t.Fatal("TextMessage should be text")
	}
	if got := messageFromWit(txt.toWit()); !got.IsText() || got.Text() != "hello" {
		t.Fatalf("text round-trip = %+v", got)
	}

	bin := BinaryMessage([]byte{1, 2, 3})
	if !bin.IsBinary() || bin.IsText() {
		t.Fatal("BinaryMessage should be binary")
	}
	got := messageFromWit(bin.toWit())
	if !got.IsBinary() || len(got.Bytes()) != 3 || got.Bytes()[2] != 3 {
		t.Fatalf("binary round-trip = %+v", got)
	}
}

// TestErrorMapping — each host error variant maps to the right kind/message.
func TestErrorMapping(t *testing.T) {
	cases := []struct {
		raw  client.Error
		kind ErrorKind
		want string
	}{
		{client.MakeErrorConnectionFailure("refused"), ConnectionFailure, "refused"},
		{client.MakeErrorSendFailure("send"), SendFailure, "send"},
		{client.MakeErrorReceiveFailure("recv"), ReceiveFailure, "recv"},
		{client.MakeErrorProtocolError("proto"), ProtocolError, "proto"},
		{client.MakeErrorOther("other"), Other, "other"},
		{client.MakeErrorSessionLost(), SessionLost, "session lost"},
	}
	for _, c := range cases {
		err := wsError(c.raw).(*Error)
		if err.Kind != c.kind {
			t.Fatalf("%v kind = %d, want %d", c.want, err.Kind, c.kind)
		}
		if err.Message != c.want {
			t.Fatalf("message = %q, want %q", err.Message, c.want)
		}
	}

	// Closed with a close frame carries the code/reason.
	closed := wsError(client.MakeErrorClosed(witTypes.Some(client.CloseInfo{Code: 1001, Reason: "bye"}))).(*Error)
	if !closed.IsClosed() || closed.CloseCode != 1001 || closed.CloseReason != "bye" {
		t.Fatalf("closed = %+v", closed)
	}
}

// Without WithReconstructionPolicy, Connect sends no policy, so the host default
// applies; with it, Connect sends exactly that policy.
func TestReconstructionPolicyOption(t *testing.T) {
	var absent connectOpts
	if absent.policy.IsSome() {
		t.Fatalf("no option must send no policy, got %v", absent.policy.Some())
	}
	for _, c := range []struct {
		policy ReconstructionPolicy
		want   client.ReconstructionPolicy
	}{
		{ReconnectAutomatically, client.ReconstructionPolicyReconnectAutomatically},
		{ReportConnectionLoss, client.ReconstructionPolicyReportConnectionLoss},
	} {
		var o connectOpts
		WithReconstructionPolicy(c.policy)(&o)
		if !o.policy.IsSome() || o.policy.Some() != c.want {
			t.Fatalf("policy %d sent %+v, want %d", c.policy, o.policy, c.want)
		}
	}
}

// A timeout of zero or less does not wait, instead of wrapping to an endless one.
func TestNonPositiveTimeoutDoesNotWait(t *testing.T) {
	for _, d := range []time.Duration{-time.Second, 0, time.Microsecond} {
		if got := timeoutMillis(d); got != 0 {
			t.Errorf("timeout %v gave %d ms", d, got)
		}
	}
	if got := timeoutMillis(1500 * time.Millisecond); got != 1500 {
		t.Errorf("1.5s gave %d ms", got)
	}
}
