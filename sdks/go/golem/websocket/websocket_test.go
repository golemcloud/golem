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
