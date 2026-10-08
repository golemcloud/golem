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

package bridge

import (
	"bytes"
	"encoding/binary"
	"encoding/json"
	"errors"
	"fmt"
	"regexp"
	"slices"
	"strconv"

	"github.com/golemcloud/golem/sdks/go/core/schema"
)

// The invocation-session public protocol, version 1, as
// docs/src/content/next/invoke/stream-session-public-protocol-v1.mdx freezes
// it: strict JSON text messages and length-prefixed binary frames.

const (
	sessionSubprotocol = "golem.agent-invocation.v1"
	sessionEndpoint    = "/v1/agents/invoke-agent-session"

	maxMessageBytes  = 32 << 20
	maxMetadataBytes = 16 << 10
	maxPackedBytes   = 1 << 20
	maxBinaryBytes   = 16 << 20
	maxTokenBytes    = 8192
	maxMappings      = 4096
	maxQueuedItems   = 256
	maxQueuedBytes   = 16 << 20
	maxSessionQueued = 32 << 20
	maxUnacked       = 16 << 20
)

// protocolError is a message the server should never have sent. It ends the
// session: retrying cannot make a malformed message well-formed.
type protocolError struct{ msg string }

func (e *protocolError) Error() string { return "golem: invocation session protocol error: " + e.msg }

func protocolErrorf(format string, args ...any) error {
	return &protocolError{msg: fmt.Sprintf(format, args...)}
}

// message is a decoded text message: its members, still raw.
type message map[string]json.RawMessage

// parseMessage reads a server text message, checks the shared envelope and
// returns its type.
func parseMessage(data []byte) (string, message, error) {
	if err := schema.CheckWireJSON(data); err != nil {
		return "", nil, protocolErrorf("malformed message: %v", err)
	}
	var m message
	if err := json.Unmarshal(data, &m); err != nil || m == nil {
		return "", nil, protocolErrorf("a message must be a JSON object")
	}
	version, ok := m["version"]
	if !ok || string(version) != "1" {
		return "", nil, protocolErrorf("unsupported or missing protocol version %s", version)
	}
	kind, err := m.str("type")
	if err != nil {
		return "", nil, err
	}
	return kind, m, nil
}

// members checks that the message carries the required members, may carry the
// optional ones, and nothing else besides type and version.
func (m message) members(what string, required []string, optional ...string) error {
	for _, name := range required {
		if _, ok := m[name]; !ok {
			return protocolErrorf("%s is missing %q", what, name)
		}
	}
	for name := range m {
		if name == "type" || name == "version" || slices.Contains(required, name) || slices.Contains(optional, name) {
			continue
		}
		return protocolErrorf("%s has unknown member %q", what, name)
	}
	return nil
}

func (m message) has(name string) bool {
	_, ok := m[name]
	return ok
}

func (m message) str(name string) (string, error) {
	var s string
	if err := json.Unmarshal(m[name], &s); err != nil {
		return "", protocolErrorf("%q must be a string", name)
	}
	return s, nil
}

func (m message) boolean(name string) (bool, error) {
	var b bool
	if err := json.Unmarshal(m[name], &b); err != nil {
		return false, protocolErrorf("%q must be a boolean", name)
	}
	return b, nil
}

func (m message) object(name string) (message, error) {
	var o message
	if err := json.Unmarshal(m[name], &o); err != nil || o == nil {
		return nil, protocolErrorf("%q must be an object", name)
	}
	return o, nil
}

var canonicalU64 = regexp.MustCompile(`^(0|[1-9][0-9]*)$`)

// u64 reads a canonical unsigned decimal string.
func (m message) u64(name string) (uint64, error) {
	s, err := m.str(name)
	if err != nil {
		return 0, err
	}
	if !canonicalU64.MatchString(s) {
		return 0, protocolErrorf("%q is not a canonical decimal string: %q", name, s)
	}
	n, err := strconv.ParseUint(s, 10, 64)
	if err != nil {
		return 0, protocolErrorf("%q overflows u64", name)
	}
	return n, nil
}

// channel reads a channel number, 1 through 2^32-1.
func (m message) channel() (uint32, error) {
	raw := string(m["channel"])
	if !canonicalU64.MatchString(raw) {
		return 0, protocolErrorf("invalid channel %s", raw)
	}
	n, err := strconv.ParseUint(raw, 10, 32)
	if err != nil || n == 0 {
		return 0, protocolErrorf("invalid channel %s", raw)
	}
	return uint32(n), nil
}

func (m message) token(name string) (string, error) {
	s, err := m.str(name)
	if err != nil {
		return "", err
	}
	if err := checkToken(s); err != nil {
		return "", protocolErrorf("%q: %v", name, err)
	}
	return s, nil
}

// checkToken checks what a client can of an opaque token: it is ASCII,
// non-empty and within the size limit.
func checkToken(s string) error {
	if s == "" || len(s) > maxTokenBytes {
		return errors.New("an opaque token must be 1 to 8192 bytes")
	}
	for i := 0; i < len(s); i++ {
		if s[i] >= 0x80 {
			return errors.New("an opaque token must be ASCII")
		}
	}
	return nil
}

// mapping is one server-issued stream mapping.
type mapping struct {
	channel     uint32
	direction   string
	token       string
	provisional string
	highWater   uint64
	terminal    bool
}

func (m message) mappings() ([]mapping, error) {
	var raws []json.RawMessage
	if err := json.Unmarshal(m["mappings"], &raws); err != nil {
		return nil, protocolErrorf("mappings must be an array")
	}
	if len(raws) > maxMappings {
		return nil, protocolErrorf("too many stream mappings")
	}
	out := make([]mapping, 0, len(raws))
	channels := map[uint32]bool{}
	tokens := map[string]bool{}
	provisionals := map[string]bool{}
	for _, raw := range raws {
		var entry message
		if err := json.Unmarshal(raw, &entry); err != nil || entry == nil {
			return nil, protocolErrorf("a stream mapping must be an object")
		}
		direction, err := entry.str("direction")
		if err != nil {
			return nil, err
		}
		required := []string{"channel", "direction", "streamToken"}
		switch direction {
		case "input":
			required = append(required, "inputHighWater")
		case "output":
		default:
			return nil, protocolErrorf("invalid stream direction %q", direction)
		}
		if err := entry.membersOnly("stream mapping", required, "provisionalRef"); err != nil {
			return nil, err
		}
		mp := mapping{direction: direction}
		if mp.channel, err = entry.channel(); err != nil {
			return nil, err
		}
		if mp.token, err = entry.token("streamToken"); err != nil {
			return nil, err
		}
		if entry.has("provisionalRef") {
			if mp.provisional, err = entry.str("provisionalRef"); err != nil {
				return nil, err
			}
			if provisionals[mp.provisional] {
				return nil, protocolErrorf("duplicate provisional stream mapping")
			}
			provisionals[mp.provisional] = true
		}
		if direction == "input" {
			hw, err := entry.object("inputHighWater")
			if err != nil {
				return nil, err
			}
			if err := hw.membersOnly("inputHighWater", []string{"sequence", "terminal"}); err != nil {
				return nil, err
			}
			if mp.highWater, err = hw.u64("sequence"); err != nil {
				return nil, err
			}
			if mp.terminal, err = hw.boolean("terminal"); err != nil {
				return nil, err
			}
		}
		if channels[mp.channel] || tokens[mp.token] {
			return nil, protocolErrorf("duplicate stream mapping")
		}
		channels[mp.channel], tokens[mp.token] = true, true
		out = append(out, mp)
	}
	return out, nil
}

// membersOnly is members for a nested object, which has no type or version.
func (m message) membersOnly(what string, required []string, optional ...string) error {
	for _, name := range required {
		if _, ok := m[name]; !ok {
			return protocolErrorf("%s is missing %q", what, name)
		}
	}
	for name := range m {
		if !slices.Contains(required, name) && !slices.Contains(optional, name) {
			return protocolErrorf("%s has unknown member %q", what, name)
		}
	}
	return nil
}

// encodeMessage renders a client message canonically: members in code-point
// order, no insignificant whitespace, nothing escaped that JSON does not
// require.
func encodeMessage(kind string, members map[string]any) ([]byte, error) {
	all := make(map[string]any, len(members)+2)
	for k, v := range members {
		all[k] = v
	}
	all["type"] = kind
	all["version"] = 1
	var buf bytes.Buffer
	enc := json.NewEncoder(&buf)
	enc.SetEscapeHTML(false)
	if err := enc.Encode(all); err != nil {
		return nil, err
	}
	return bytes.TrimSuffix(buf.Bytes(), []byte("\n")), nil
}

// binaryFrame is a decoded binary message.
type binaryFrame struct {
	kind     string
	channel  uint32
	sequence uint64
	count    uint64
	cursor   string
	mime     *string
	payload  []byte
}

// encodeBinary renders a client binary frame: the metadata length as a
// big-endian u32, the metadata JSON, then the payload.
func encodeBinary(kind string, channel uint32, sequence uint64, count int, mime *string, payload []byte) ([]byte, error) {
	meta := map[string]any{
		"channel":   channel,
		"itemCount": strconv.Itoa(count),
		"kind":      kind,
		"sequence":  strconv.FormatUint(sequence, 10),
		"version":   1,
	}
	if mime != nil {
		meta["mimeType"] = *mime
	}
	var buf bytes.Buffer
	enc := json.NewEncoder(&buf)
	enc.SetEscapeHTML(false)
	if err := enc.Encode(meta); err != nil {
		return nil, err
	}
	metadata := bytes.TrimSuffix(buf.Bytes(), []byte("\n"))
	out := make([]byte, 4, 4+len(metadata)+len(payload))
	binary.BigEndian.PutUint32(out, uint32(len(metadata)))
	out = append(out, metadata...)
	return append(out, payload...), nil
}

var mimePattern = regexp.MustCompile(`^[A-Za-z0-9!#$&^_.+\-]+/[A-Za-z0-9!#$&^_.+\-]+$`)

// decodeBinary reads a server binary frame and checks its lane rules.
func decodeBinary(data []byte) (binaryFrame, error) {
	if len(data) > maxMessageBytes {
		return binaryFrame{}, protocolErrorf("binary frame exceeds the protocol limit")
	}
	if len(data) < 4 {
		return binaryFrame{}, protocolErrorf("binary frame is shorter than its length prefix")
	}
	n := binary.BigEndian.Uint32(data)
	if n > maxMetadataBytes || int(n) > len(data)-4 {
		return binaryFrame{}, protocolErrorf("malformed binary metadata length")
	}
	metadata, payload := data[4:4+n], data[4+n:]
	if err := schema.CheckWireJSON(metadata); err != nil {
		return binaryFrame{}, protocolErrorf("malformed binary metadata: %v", err)
	}
	var m message
	if err := json.Unmarshal(metadata, &m); err != nil || m == nil {
		return binaryFrame{}, protocolErrorf("binary metadata must be a JSON object")
	}
	if string(m["version"]) != "1" {
		return binaryFrame{}, protocolErrorf("unsupported binary metadata version")
	}
	kind, err := m.str("kind")
	if err != nil {
		return binaryFrame{}, err
	}
	required := []string{"channel", "itemCount", "kind", "sequence", "version"}
	var optional []string
	switch kind {
	case "output-u8", "input-u8":
	case "output-binary", "input-binary":
		optional = append(optional, "mimeType")
	default:
		return binaryFrame{}, protocolErrorf("unknown binary lane %q", kind)
	}
	if kind == "output-u8" || kind == "output-binary" {
		required = append(required, "cursorToken")
	}
	if err := m.membersOnly("binary metadata", required, optional...); err != nil {
		return binaryFrame{}, err
	}
	f := binaryFrame{kind: kind, payload: payload}
	if f.channel, err = m.channel(); err != nil {
		return binaryFrame{}, err
	}
	if f.sequence, err = m.u64("sequence"); err != nil {
		return binaryFrame{}, err
	}
	if f.count, err = m.u64("itemCount"); err != nil {
		return binaryFrame{}, err
	}
	if m.has("cursorToken") {
		if f.cursor, err = m.token("cursorToken"); err != nil {
			return binaryFrame{}, err
		}
	}
	if m.has("mimeType") {
		mime, err := m.str("mimeType")
		if err != nil {
			return binaryFrame{}, err
		}
		if !mimePattern.MatchString(mime) {
			return binaryFrame{}, protocolErrorf("illegal MIME type %q", mime)
		}
		f.mime = &mime
	}
	switch kind {
	case "output-u8", "input-u8":
		if len(payload) == 0 || len(payload) > maxPackedBytes {
			return binaryFrame{}, protocolErrorf("a packed u8 payload must be 1 byte to 1 MiB")
		}
		if f.count != uint64(len(payload)) {
			return binaryFrame{}, protocolErrorf("packed u8 item count %d does not match %d payload bytes", f.count, len(payload))
		}
	default:
		if len(payload) > maxBinaryBytes {
			return binaryFrame{}, protocolErrorf("a binary item exceeds 16 MiB")
		}
		if f.count != 1 {
			return binaryFrame{}, protocolErrorf("a binary frame carries exactly one item")
		}
	}
	if f.sequence+f.count < f.sequence {
		return binaryFrame{}, protocolErrorf("stream sequence overflows u64")
	}
	return f, nil
}

var cancelReasons = []string{
	"cancelled", "consumer-drop", "transport-detached", "source-unavailable",
	"producer-deleted", "invocation-failed", "protocol-error",
}
