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
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"maps"
	"slices"
	"strconv"
	"strings"
	"unicode/utf8"
)

// SnapshotPart is a named part of a [MultipartSnapshot]: opaque bytes with a
// bare MIME type such as "application/octet-stream".
type SnapshotPart struct {
	Bytes       []byte
	ContentType string
}

// MultipartSnapshot is a snapshot of JSON state and named binary parts. Use it
// when the state holds data that should not go through JSON, such as an index
// or an image. Binary-only state is JSON null. A part name starts with a letter,
// a digit or an underscore, and continues with those, '.' or '-'.
type MultipartSnapshot struct {
	State json.RawMessage
	Parts map[string]SnapshotPart
}

// MultipartSnapshotter lets an agent's state save itself as a
// [MultipartSnapshot]. Implement it on the state type instead of
// [Snapshotter]; a state that implements both is a definition error. LoadParts
// runs on the zero state, as Load does, and the [AgentImpl.OnRestore] hook runs
// after it.
type MultipartSnapshotter interface {
	SaveParts() (MultipartSnapshot, error)
	LoadParts(MultipartSnapshot) error
}

// NewMultipartSnapshot builds a snapshot from the JSON of state and the parts.
func NewMultipartSnapshot(state any, parts map[string]SnapshotPart) (MultipartSnapshot, error) {
	s, err := json.Marshal(state)
	if err != nil {
		return MultipartSnapshot{}, err
	}
	return MultipartSnapshot{State: s, Parts: parts}, nil
}

// DecodeState decodes the snapshot's JSON state into v.
func (s MultipartSnapshot) DecodeState(v any) error { return json.Unmarshal(s.State, v) }

// RequirePart returns the bytes of the part name, which must have the content
// type contentType. MIME types compare case-insensitively.
func (s MultipartSnapshot) RequirePart(name, contentType string) ([]byte, error) {
	part, ok := s.Parts[name]
	if !ok {
		return nil, fmt.Errorf("missing required part %q", name)
	}
	got, err := normalizeContentType(part.ContentType)
	if err != nil {
		return nil, err
	}
	want, err := normalizeContentType(contentType)
	if err != nil {
		return nil, err
	}
	if got != want {
		return nil, fmt.Errorf("part %q has unexpected Content-Type %q", name, part.ContentType)
	}
	return part.Bytes, nil
}

// The wire form is multipart/mixed, shared by every SDK
// (test-data/snapshot-multipart/README.md): one "state" part with the
// {"version":1,"principal":…,"state":…} envelope, and one "part:<name>" part
// per user part.
const (
	multipartMIME         = "multipart/mixed"
	multipartStatePart    = "state"
	multipartUserPrefix   = "part:"
	multipartBoundaryBase = "golem-snapshot-"
)

func validPartName(name string) bool {
	if name == "" {
		return false
	}
	return (isAlnum(name[0]) || name[0] == '_') && onlyAlnumOr(name[1:], "_.-")
}

func isAlnum(b byte) bool {
	return b >= 'a' && b <= 'z' || b >= 'A' && b <= 'Z' || b >= '0' && b <= '9'
}

// onlyAlnumOr reports whether every byte of s is an ASCII letter, a digit or
// one of extra.
func onlyAlnumOr(s, extra string) bool {
	for i := 0; i < len(s); i++ {
		if !isAlnum(s[i]) && strings.IndexByte(extra, s[i]) < 0 {
			return false
		}
	}
	return true
}

func isMIMEToken(s string) bool { return s != "" && onlyAlnumOr(s, "!#$%&'*+.^_`|~-") }

// normalizeContentType accepts a bare type/subtype and lower-cases it.
func normalizeContentType(value string) (string, error) {
	typ, sub, ok := strings.Cut(value, "/")
	if !ok || !isMIMEToken(typ) || !isMIMEToken(sub) {
		return "", fmt.Errorf("invalid snapshot Content-Type %q", value)
	}
	return strings.ToLower(value), nil
}

func validBoundary(b string) bool {
	return b != "" && len(b) <= 70 && onlyAlnumOr(b, "'()+_,./:=?-")
}

// extractBoundary returns the boundary of a multipart/mixed MIME type, or false
// if mime is not one this SDK accepts.
func extractBoundary(mime string) (string, bool) {
	params := strings.Split(mime, ";")
	if len(params) != 2 || !strings.EqualFold(strings.TrimSpace(params[0]), multipartMIME) {
		return "", false
	}
	key, value, ok := strings.Cut(params[1], "=")
	if !ok || !strings.EqualFold(strings.TrimSpace(key), "boundary") {
		return "", false
	}
	value = strings.TrimSpace(value)
	if strings.HasPrefix(value, `"`) {
		if len(value) < 2 || !strings.HasSuffix(value, `"`) {
			return "", false
		}
		value = value[1 : len(value)-1]
	} else if !onlyAlnumOr(value, "'+_.-") {
		return "", false
	}
	return value, validBoundary(value)
}

// delimiter matches a delimiter line at pos: the marker, an optional "--", and
// the newline, or EOF after a closing delimiter. It returns the position after
// the line and whether the delimiter closes the body.
func delimiter(data, marker, newline []byte, pos int) (int, bool, bool) {
	if pos > len(data) || !bytes.HasPrefix(data[pos:], marker) {
		return 0, false, false
	}
	end := pos + len(marker)
	closing := bytes.HasPrefix(data[end:], []byte("--"))
	if closing {
		end += 2
	}
	switch {
	case bytes.HasPrefix(data[end:], newline):
		return end + len(newline), closing, true
	case closing && end == len(data):
		return end, true, true
	}
	return 0, false, false
}

type wirePart struct {
	name        string
	contentType string
	body        []byte
}

// parseParts splits a multipart body into its parts, or returns false if the
// framing or a part's headers are malformed.
func parseParts(boundary string, data []byte) ([]wirePart, bool) {
	if !validBoundary(boundary) {
		return nil, false
	}
	marker := []byte("--" + boundary)
	start := 0
	switch {
	case bytes.HasPrefix(data, []byte("\r\n")):
		start = 2
	case bytes.HasPrefix(data, []byte("\n")):
		start = 1
	}
	suffix := start + len(marker)
	if suffix+2 <= len(data) && string(data[suffix:suffix+2]) == "--" {
		suffix += 2
	}
	newline := []byte("\n")
	if suffix+2 <= len(data) && string(data[suffix:suffix+2]) == "\r\n" || suffix == len(data) && start == 2 {
		newline = []byte("\r\n")
	}
	if start != 0 && start != len(newline) {
		return nil, false
	}
	pos, closing, ok := delimiter(data, marker, newline, start)
	if !ok {
		return nil, false
	}
	var parts []wirePart
	names := map[string]bool{}
	for !closing {
		var name, contentType *string
		headers := map[string]bool{}
		for {
			nl := bytes.IndexByte(data[pos:], '\n')
			if nl < 0 {
				return nil, false
			}
			line := bytes.TrimSuffix(data[pos:pos+nl], []byte("\r"))
			pos += nl + 1
			if len(line) == 0 {
				break
			}
			if line[0] == ' ' || slices.ContainsFunc(line, func(b byte) bool { return b < 32 || b > 126 }) {
				return nil, false
			}
			k, v, ok := strings.Cut(string(line), ":")
			if !ok {
				return nil, false
			}
			k = strings.ToLower(k)
			if headers[k] {
				return nil, false
			}
			headers[k] = true
			v = strings.TrimSpace(v)
			switch k {
			case "content-type":
				contentType = &v
			case "content-disposition":
				n, ok := dispositionName(v)
				if !ok {
					return nil, false
				}
				name = &n
			default:
				return nil, false
			}
		}
		if name == nil || contentType == nil || names[*name] {
			return nil, false
		}
		names[*name] = true
		bodyEnd, next := -1, 0
		for i := pos; i < len(data); i++ {
			if !bytes.HasPrefix(data[i:], newline) {
				continue
			}
			if end, close, ok := delimiter(data, marker, newline, i+len(newline)); ok {
				bodyEnd, next, closing = i, end, close
				break
			}
		}
		if bodyEnd < 0 {
			return nil, false
		}
		parts = append(parts, wirePart{name: *name, contentType: *contentType, body: data[pos:bodyEnd]})
		pos = next
	}
	return parts, pos == len(data)
}

// dispositionName reads the name of `attachment; name="..."`.
func dispositionName(v string) (string, bool) {
	kind, param, ok := strings.Cut(v, ";")
	if !ok || !strings.EqualFold(kind, "attachment") {
		return "", false
	}
	key, value, ok := strings.Cut(strings.TrimSpace(param), "=")
	if !ok || !strings.EqualFold(key, "name") || len(value) < 2 || value[0] != '"' || value[len(value)-1] != '"' {
		return "", false
	}
	value = value[1 : len(value)-1]
	if value == "" || strings.ContainsAny(value, `"\`) {
		return "", false
	}
	return value, true
}

// encodeMultipart frames a multipart snapshot with the principal and returns
// the payload and its MIME type.
func encodeMultipart(snap MultipartSnapshot, principal []byte) ([]byte, string, error) {
	state := snap.State
	if state == nil {
		state = json.RawMessage("null")
	}
	if !json.Valid(state) {
		return nil, "", errors.New("golem: the multipart snapshot's state is not valid JSON")
	}
	envelope := slices.Concat([]byte(`{"version":1,"principal":`), principal, []byte(`,"state":`), state, []byte("}"))
	type framed struct {
		name, contentType string
		body              []byte
	}
	parts := []framed{{multipartStatePart, snapshotJSONMIME, envelope}}
	for _, name := range slices.Sorted(maps.Keys(snap.Parts)) {
		if !validPartName(name) {
			return nil, "", fmt.Errorf("golem: invalid snapshot part name %q", name)
		}
		ct, err := normalizeContentType(snap.Parts[name].ContentType)
		if err != nil {
			return nil, "", fmt.Errorf("golem: %w", err)
		}
		parts = append(parts, framed{multipartUserPrefix + name, ct, snap.Parts[name].Bytes})
	}
	crlf := []byte("\r\n")
	var boundary string
	for nonce := 0; ; nonce++ {
		boundary = multipartBoundaryBase + strconv.Itoa(nonce)
		marker := []byte("--" + boundary)
		collides := slices.ContainsFunc(parts, func(p framed) bool {
			body := slices.Concat(p.body, crlf)
			if _, _, ok := delimiter(body, marker, crlf, 0); ok {
				return true
			}
			for i := range body {
				if bytes.HasPrefix(body[i:], crlf) {
					if _, _, ok := delimiter(body, marker, crlf, i+2); ok {
						return true
					}
				}
			}
			return false
		})
		if !collides {
			break
		}
	}
	var out bytes.Buffer
	for _, p := range parts {
		fmt.Fprintf(&out, "--%s\r\nContent-Disposition: attachment; name=\"%s\"\r\nContent-Type: %s\r\n\r\n", boundary, p.name, p.contentType)
		out.Write(p.body)
		out.Write(crlf)
	}
	fmt.Fprintf(&out, "--%s--\r\n", boundary)
	return out.Bytes(), multipartMIME + "; boundary=" + boundary, nil
}

var errInvalidMultipart = errors.New("golem: invalid multipart snapshot")

// decodeMultipart reads a multipart snapshot and the principal JSON it carries.
func decodeMultipart(data []byte, mime string) (json.RawMessage, MultipartSnapshot, error) {
	boundary, ok := extractBoundary(mime)
	if !ok {
		return nil, MultipartSnapshot{}, errInvalidMultipart
	}
	parts, ok := parseParts(boundary, data)
	if !ok {
		return nil, MultipartSnapshot{}, errInvalidMultipart
	}
	i := slices.IndexFunc(parts, func(p wirePart) bool { return p.name == multipartStatePart })
	if i < 0 || parts[i].contentType != snapshotJSONMIME {
		return nil, MultipartSnapshot{}, errInvalidMultipart
	}
	envelope, err := uniqueObjectFields(parts[i].body)
	if err != nil {
		return nil, MultipartSnapshot{}, fmt.Errorf("golem: the multipart envelope: %w", err)
	}
	if string(envelope["version"]) != "1" || envelope["principal"] == nil || envelope["state"] == nil {
		return nil, MultipartSnapshot{}, errors.New("golem: the multipart envelope needs version 1, a principal and a state")
	}
	principal := envelope["principal"]
	if _, err := uniqueObjectFields(principal); err != nil {
		return nil, MultipartSnapshot{}, fmt.Errorf("golem: the multipart principal: %w", err)
	}
	snap := MultipartSnapshot{State: envelope["state"], Parts: map[string]SnapshotPart{}}
	for _, p := range parts {
		if p.name == multipartStatePart {
			continue
		}
		name, ok := strings.CutPrefix(p.name, multipartUserPrefix)
		if !ok || !validPartName(name) {
			return nil, MultipartSnapshot{}, errInvalidMultipart
		}
		ct, err := normalizeContentType(p.contentType)
		if err != nil {
			return nil, MultipartSnapshot{}, fmt.Errorf("golem: %w", err)
		}
		snap.Parts[name] = SnapshotPart{Bytes: p.body, ContentType: ct}
	}
	return principal, snap, nil
}

// uniqueObjectFields reads a UTF-8 JSON object into its raw fields, rejecting
// a duplicate key, which encoding/json would silently resolve.
func uniqueObjectFields(data []byte) (map[string]json.RawMessage, error) {
	if !utf8.Valid(data) {
		return nil, errors.New("not UTF-8")
	}
	dec := json.NewDecoder(bytes.NewReader(data))
	if tok, err := dec.Token(); err != nil || tok != json.Delim('{') {
		return nil, errors.New("not a JSON object")
	}
	fields := map[string]json.RawMessage{}
	for dec.More() {
		tok, err := dec.Token()
		if err != nil {
			return nil, err
		}
		key := tok.(string)
		if _, dup := fields[key]; dup {
			return nil, fmt.Errorf("duplicate key %q", key)
		}
		var raw json.RawMessage
		if err := dec.Decode(&raw); err != nil {
			return nil, err
		}
		fields[key] = raw
	}
	if _, err := dec.Token(); err != nil {
		return nil, err
	}
	if _, err := dec.Token(); err == nil {
		return nil, errors.New("trailing data after the object")
	}
	return fields, nil
}
