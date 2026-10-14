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
	"encoding/hex"
	"encoding/json"
	"fmt"
	"os"
	"strings"
	"testing"

	host "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_api_host"
)

type multipartIndex struct {
	revision uint32
	index    []byte
	restored bool
}

type savedIndex struct {
	Revision uint32 `json:"revision"`
}

func (s *multipartIndex) SaveParts() (MultipartSnapshot, error) {
	return NewMultipartSnapshot(savedIndex{Revision: s.revision}, map[string]SnapshotPart{
		"index": {Bytes: s.index, ContentType: "application/octet-stream"},
	})
}

func (s *multipartIndex) LoadParts(snap MultipartSnapshot) error {
	var saved savedIndex
	if err := snap.DecodeState(&saved); err != nil {
		return err
	}
	index, err := snap.RequirePart("index", "application/octet-stream")
	if err != nil {
		return err
	}
	*s = multipartIndex{revision: saved.Revision, index: index, restored: true}
	return nil
}

func allBytes() []byte {
	b := make([]byte, 256)
	for i := range b {
		b[i] = byte(i)
	}
	return b
}

func TestMultipartSnapshotRoundTripsStatePartsAndPrincipal(t *testing.T) {
	src := &multipartIndex{revision: 13, index: allBytes()}
	snap, err := saveState(src, snapPrincipal)
	if err != nil {
		t.Fatalf("save: %v", err)
	}
	if !strings.HasPrefix(snap.MimeType, "multipart/mixed; boundary=") {
		t.Fatalf("mime %q", snap.MimeType)
	}
	dst := &multipartIndex{}
	principal := restore(t, dst, snap)
	if fmt.Sprint(principal) != fmt.Sprint(snapPrincipal) {
		t.Errorf("principal %v, want %v", principal, snapPrincipal)
	}
	if dst.revision != 13 || !bytes.Equal(dst.index, src.index) || !dst.restored {
		t.Errorf("restored %+v", dst)
	}
}

func TestMultipartSnapshotKeepsDynamicOpaqueParts(t *testing.T) {
	parts := map[string]SnapshotPart{
		"index":     {Bytes: allBytes(), ContentType: "Application/Octet-Stream"},
		"__proto__": {Bytes: []byte{}, ContentType: "Application/Octet-Stream"},
		"state":     {Bytes: []byte{0, 255, 13, 10}, ContentType: "Application/Octet-Stream"},
	}
	snap, err := NewMultipartSnapshot(map[string]any{"revision": 17, "text": "Árvíz 🦀"}, parts)
	if err != nil {
		t.Fatal(err)
	}
	data, mime, err := encodeMultipart(snap, []byte("{}"))
	if err != nil {
		t.Fatal(err)
	}
	principal, decoded, err := decodeMultipart(data, mime)
	if err != nil {
		t.Fatal(err)
	}
	if string(principal) != "{}" || string(decoded.State) != string(snap.State) || len(decoded.Parts) != 3 {
		t.Fatalf("decoded %s %s %d", principal, decoded.State, len(decoded.Parts))
	}
	for name, part := range parts {
		got, err := decoded.RequirePart(name, "APPLICATION/OCTET-STREAM")
		if err != nil || !bytes.Equal(got, part.Bytes) {
			t.Errorf("part %q: %v %v", name, got, err)
		}
	}
	for _, c := range []struct{ name, contentType string }{
		{"missing", "text/plain"},
		{"index", "text/plain"},
		{"index", "application/octet-stream; charset=utf-8"},
	} {
		if _, err := decoded.RequirePart(c.name, c.contentType); err == nil {
			t.Errorf("RequirePart(%q, %q) succeeded", c.name, c.contentType)
		}
	}

	empty := MultipartSnapshot{State: json.RawMessage("null")}
	data, mime, err = encodeMultipart(empty, []byte("{}"))
	if err != nil {
		t.Fatal(err)
	}
	if _, got, err := decodeMultipart(data, mime); err != nil || string(got.State) != "null" || len(got.Parts) != 0 {
		t.Errorf("empty snapshot: %v %+v", err, got)
	}
}

func TestMultipartSharedFramingFixtures(t *testing.T) {
	raw, err := os.ReadFile("../../../test-data/snapshot-multipart/framing.json")
	if err != nil {
		t.Fatal(err)
	}
	var fixtures struct {
		Boundary string `json:"boundary"`
		Valid    []struct {
			Name    string `json:"name"`
			Newline string `json:"newline"`
			Payload string `json:"payload"`
			Hex     string `json:"hex"`
		} `json:"valid"`
	}
	if err := json.Unmarshal(raw, &fixtures); err != nil {
		t.Fatal(err)
	}
	if len(fixtures.Valid) == 0 {
		t.Fatal("no framing fixtures")
	}
	for _, f := range fixtures.Valid {
		nl, b := f.Newline, fixtures.Boundary
		data := "--" + b + nl + `Content-Disposition: attachment; name="part:opaque"` + nl +
			"Content-Type: application/octet-stream" + nl + nl + f.Payload + nl + "--" + b + "--" + nl
		parts, ok := parseParts(b, []byte(data))
		want, _ := hex.DecodeString(f.Hex)
		if !ok || len(parts) != 1 || !bytes.Equal(parts[0].body, want) {
			t.Errorf("%s: parts %v ok %v", f.Name, parts, ok)
		}
	}
}

func TestMultipartBoundaryAvoidsCollisions(t *testing.T) {
	body := []byte("--golem-snapshot-0\r\npayload\r\n--golem-snapshot-1--")
	snap := MultipartSnapshot{
		State: json.RawMessage("null"),
		Parts: map[string]SnapshotPart{"index": {Bytes: body, ContentType: "application/octet-stream"}},
	}
	data, mime, err := encodeMultipart(snap, []byte("{}"))
	if err != nil {
		t.Fatal(err)
	}
	if b, _ := extractBoundary(mime); b != "golem-snapshot-2" {
		t.Errorf("boundary %q", b)
	}
	_, decoded, err := decodeMultipart(data, mime)
	if err != nil {
		t.Fatal(err)
	}
	if got, err := decoded.RequirePart("index", "application/octet-stream"); err != nil || !bytes.Equal(got, body) {
		t.Errorf("index %q %v", got, err)
	}
}

func TestMultipartRejectsMalformedFramingEnvelopesAndNames(t *testing.T) {
	const mime = "multipart/mixed; boundary=b"
	envelope := `{"version":1,"principal":{},"state":null}`
	wrap := func(state, extra string) []byte {
		return []byte("--b\r\nContent-Type: application/json\r\nContent-Disposition: attachment; name=\"state\"\r\n\r\n" + state + "\r\n" + extra + "--b--\r\n")
	}
	valid := string(wrap(envelope, ""))
	if _, _, err := decodeMultipart([]byte(valid), mime); err != nil {
		t.Fatalf("valid: %v", err)
	}
	if _, _, err := decodeMultipart(wrap(strings.Replace(envelope, "null", "[17,null]", 1), ""), mime); err != nil {
		t.Errorf("array state: %v", err)
	}
	for _, state := range []string{
		`[1,{},null]`,
		`{"version":1,"principal":["anonymous"],"state":null}`,
		strings.Replace(envelope, `"version":1`, `"version":1,"version":1`, 1),
		strings.Replace(envelope, `"version":1`, `"version":1.0`, 1),
		strings.Replace(envelope, `"version":1`, `"version":1e0`, 1),
		strings.Replace(envelope, `"principal":{}`, `"principal":{"agent":{},"agent":{}}`, 1),
		`{"version":1,"principal":{}}`,
	} {
		if _, _, err := decodeMultipart(wrap(state, ""), mime); err == nil {
			t.Errorf("accepted envelope %s", state)
		}
	}
	for _, name := range []string{"part:", "part:a/b", "db:main", "unknown:x"} {
		extra := "--b\r\nContent-Type: application/octet-stream\r\nContent-Disposition: attachment; name=\"" + name + "\"\r\n\r\nx\r\n"
		if _, _, err := decodeMultipart(wrap(envelope, extra), mime); err == nil {
			t.Errorf("accepted part name %q", name)
		}
	}
	for _, data := range []string{
		strings.Replace(valid, "--b--\r\n", "", 1),
		valid + "epilogue",
		strings.Replace(valid, "Content-Type: application/json", "Content-Type: application/json\r\nContent-Type: application/json", 1),
		strings.Replace(valid, "Content-Type: application/json", "Content-Type: application/json\r\nUnknown: x", 1),
	} {
		if _, _, err := decodeMultipart([]byte(data), mime); err == nil {
			t.Errorf("accepted framing %q", data)
		}
	}
	for _, c := range []struct{ name, contentType string }{
		{"a/b", "text/plain"},
		{"index", "text/plain; charset=utf-8"},
		{"index", "text/plain\r\nHeader: injected"},
	} {
		snap := MultipartSnapshot{State: json.RawMessage("null"), Parts: map[string]SnapshotPart{c.name: {ContentType: c.contentType}}}
		if _, _, err := encodeMultipart(snap, []byte("{}")); err == nil {
			t.Errorf("encoded part %q with %q", c.name, c.contentType)
		}
	}
	notUTF8 := []byte(valid)
	notUTF8[bytes.Index(notUTF8, []byte("null"))] = 255
	if _, _, err := decodeMultipart(notUTF8, mime); err == nil {
		t.Error("accepted a non-UTF-8 envelope")
	}
}

func TestMultipartAndSimpleModesDoNotMix(t *testing.T) {
	multipart, err := saveState(&multipartIndex{index: []byte{1}}, snapPrincipal)
	if err != nil {
		t.Fatal(err)
	}
	_, state, err := splitSnapshot(multipart)
	if err != nil {
		t.Fatal(err)
	}
	if err := loadState(&snapCustomState{}, state); err == nil {
		t.Error("a Snapshotter state loaded a multipart snapshot")
	}
	if err := loadState(&struct{ Count int }{}, state); err == nil {
		t.Error("a JSON state loaded a multipart snapshot and dropped its parts")
	}
	plain, err := saveState(&snapCustomState{count: 1}, snapPrincipal)
	if err != nil {
		t.Fatal(err)
	}
	_, state, err = splitSnapshot(plain)
	if err != nil {
		t.Fatal(err)
	}
	if err := loadState(&multipartIndex{}, state); err == nil {
		t.Error("a MultipartSnapshotter state loaded a raw snapshot")
	}
	if _, _, err := splitSnapshot(host.Snapshot{Payload: []byte("x"), MimeType: "multipart/mixed; boundary=b"}); err == nil {
		t.Error("split a malformed multipart snapshot")
	}
}

type bothSnapshotters struct{ multipartIndex }

func (*bothSnapshotters) Save() ([]byte, error) { return nil, nil }
func (*bothSnapshotters) Load([]byte) error     { return nil }

func TestStateImplementingBothSnapshottersIsADefinitionError(t *testing.T) {
	withDefs(t, func(d *definitions) {
		def := defineAgentInto[restoreID, NoConfig](d, Spec{Name: "Both"})
		implementInto[restoreID, bothSnapshotters, NoConfig](d, def, simpleNewState[restoreID, bothSnapshotters](func(restoreID) *bothSnapshotters {
			return &bothSnapshotters{}
		}), false)
		mustDefErr(t, d, "both Snapshotter and MultipartSnapshotter")
	})
}
