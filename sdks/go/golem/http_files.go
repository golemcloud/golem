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
	"net/url"
	"strings"
	"unicode/utf8"

	common "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_common"
)

// FileMapping serves a file, or a directory tree, from the agent's filesystem
// at an HTTP route under its mount.
//
// An exact mapping names one file:
//
//	{Route: "/value", Path: "/value.txt"}
//
// A subtree mapping ends its route in "/*" and its path in "/$1", where $1 is the
// rest of the request path:
//
//	{Route: "/assets/*", Path: "/public/$1"}
//
// Mappings are tried in order, so a route may repeat with a different path as a
// fallback. Only GET and HEAD are served; there is no directory listing, no
// implicit index file, and no symlink following.
type FileMapping struct {
	Route string
	Path  string
}

// compileFileMappings turns mappings into their WIT form. A mapping that does not
// compile yields an error naming it and one of the shared contract's categories
// (source-path, source-wildcard, target-placeholder, target-path,
// duplicate-mapping), which every SDK reports alike.
func compileFileMappings(mappings []FileMapping) ([]common.FileMapping, []string) {
	out := make([]common.FileMapping, 0, len(mappings))
	var errs []string
	seen := map[string]bool{}
	for i, m := range mappings {
		compiled, key, category := compileFileMapping(m)
		if category == "" && seen[key] {
			category = "duplicate-mapping"
		}
		if category != "" {
			errs = append(errs, fmt.Sprintf("file mapping %d (%q -> %q): %s", i, m.Route, m.Path, category))
			continue
		}
		seen[key] = true
		out = append(out, compiled)
	}
	return out, errs
}

func compileFileMapping(m FileMapping) (common.FileMapping, string, string) {
	subtree := strings.HasSuffix(m.Route, "/*")
	source := m.Route
	if subtree {
		source = strings.TrimSuffix(m.Route, "/*")
		if source == "" {
			source = "/"
		}
	}
	if strings.Contains(source, "*") {
		return common.FileMapping{}, "", "source-wildcard"
	}
	if subtree && source != "/" && strings.HasSuffix(source, "/") {
		return common.FileMapping{}, "", "source-path"
	}
	// "//*" is not how the root subtree is spelled.
	if subtree && m.Route != "/*" && source == "/" {
		return common.FileMapping{}, "", "source-path"
	}
	segments, ok := publicSegments(source)
	if !ok {
		return common.FileMapping{}, "", "source-path"
	}

	if subtree && !strings.HasSuffix(m.Path, "/$1") {
		return common.FileMapping{}, "", "target-placeholder"
	}
	target := m.Path
	if subtree {
		target = strings.TrimSuffix(m.Path, "/$1")
		if target == "" {
			target = "/"
		}
	}
	if strings.Contains(target, "$") {
		return common.FileMapping{}, "", "target-placeholder"
	}
	if !validTarget(target, subtree) || (subtree && m.Path != "/$1" && target == "/") {
		return common.FileMapping{}, "", "target-path"
	}

	key := fmt.Sprintf("%t\x00%s\x00%s", subtree, strings.Join(segments, "\x00"), target)
	if subtree {
		return common.MakeFileMappingSubtree(common.SubtreeFileMapping{PublicPrefix: segments, FilesystemRoot: target}), key, ""
	}
	return common.MakeFileMappingExact(common.ExactFileMapping{PublicPath: segments, FilePath: target}), key, ""
}

// publicSegments splits a route into its decoded segments. A route is percent-
// encoded exactly once: "/%61" and "/a" are the same route.
func publicSegments(route string) ([]string, bool) {
	if route == "/" {
		return []string{}, true
	}
	if !strings.HasPrefix(route, "/") || strings.ContainsAny(route, "$*?#") {
		return nil, false
	}
	raw := strings.Split(route[1:], "/")
	out := make([]string, 0, len(raw))
	for _, r := range raw {
		if !validRawSegment(r) {
			return nil, false
		}
		decoded, err := url.PathUnescape(r)
		if err != nil || !validSegment(decoded) {
			return nil, false
		}
		out = append(out, decoded)
	}
	return out, true
}

// validRawSegment accepts the unreserved and sub-delimiter characters of a URI
// path segment, plus percent escapes.
func validRawSegment(s string) bool {
	if s == "" {
		return false
	}
	for i := 0; i < len(s); i++ {
		c := s[i]
		switch {
		case c >= 'A' && c <= 'Z', c >= 'a' && c <= 'z', c >= '0' && c <= '9':
		case strings.IndexByte("-._~!&'()+,;=:@", c) >= 0:
		case c == '%':
			if i+2 >= len(s) || !isHex(s[i+1]) || !isHex(s[i+2]) {
				return false
			}
			i += 2
		default:
			return false
		}
	}
	return true
}

func isHex(c byte) bool {
	return (c >= '0' && c <= '9') || (c >= 'a' && c <= 'f') || (c >= 'A' && c <= 'F')
}

func validSegment(s string) bool {
	if s == "" || s == "." || s == ".." || !utf8.ValidString(s) {
		return false
	}
	for _, r := range s {
		if r < 0x20 || r == 0x7f || r == '/' || r == '\\' {
			return false
		}
	}
	return true
}

// validTarget checks a filesystem path. It is plain text, never URI-decoded:
// "/%2e%2e" is a file with that name, not a parent reference.
func validTarget(target string, subtree bool) bool {
	if !strings.HasPrefix(target, "/") || (!subtree && target == "/") || !utf8.ValidString(target) {
		return false
	}
	for _, r := range target {
		if r < 0x20 || (r >= 0x7f && r <= 0x9f) {
			return false
		}
	}
	if target == "/" {
		return true
	}
	for _, s := range strings.Split(target[1:], "/") {
		if !validSegment(s) {
			return false
		}
	}
	return true
}

// validateFileOwner checks that an agent may expose its own files: it must be a
// durable, non-phantom agent, and its identity must come entirely from the
// mount path — each constructor field a scalar, captured exactly once — so a
// request can name the one agent whose files it reads.
func validateFileOwner(e *agentEntry, mp parsedPath) []string {
	var errs []string
	if e.mode != common.AgentModeDurable || e.mount.PhantomAgent {
		errs = append(errs, "ExposeFiles needs a durable, non-phantom agent: filesystem-owner")
	}
	captured := map[string]int{}
	for _, s := range mp.segs {
		switch s.kind {
		case varSeg:
			captured[s.value]++
		case restSeg:
			errs = append(errs, fmt.Sprintf("ExposeFiles cannot be used with the catch-all mount variable {*%s}: unbound-constructor", s.value))
		}
	}
	for _, f := range e.idFields {
		if captured[f.name] != 1 {
			errs = append(errs, fmt.Sprintf("ExposeFiles needs Id field %q captured exactly once by the mount path: unbound-constructor", f.name))
			continue
		}
		if !bindableKind(f.typ.Kind()) {
			errs = append(errs, fmt.Sprintf("ExposeFiles needs scalar Id fields, but %q is %s: unbound-constructor", f.name, f.typ))
		}
	}
	return errs
}
