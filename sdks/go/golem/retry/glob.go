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

package retry

// globMatch reports whether path matches glob, with the semantics of the
// glob-match crate the host evaluates retry predicates with: `?`, `*` (not
// across '/'), `**` (whole path segments), `[...]` classes with ranges and
// `!`/`^` negation, `{a,b}` alternatives nested up to ten deep, `\` escapes,
// and leading `!` negating the whole pattern. A malformed pattern matches
// nothing. It works on bytes, as the crate does.
func globMatch(glob, path string) bool {
	var s globState
	var braces braceStack

	negated := false
	for s.glob < len(glob) && glob[s.glob] == '!' {
		negated = !negated
		s.glob++
	}

	for s.glob < len(glob) || s.path < len(path) {
		if s.glob < len(glob) {
			switch c := glob[s.glob]; {
			case c == '*':
				globstar := s.glob+1 < len(glob) && glob[s.glob+1] == '*'
				if globstar {
					s.glob = skipGlobstars(glob, s.glob+2) - 2
				}
				s.wildcard.glob = s.glob
				s.wildcard.path = s.path + 1

				if globstar {
					s.glob += 2
					if len(glob) == s.glob {
						s.globstar = s.wildcard
					} else if (s.glob < 3 || glob[s.glob-3] == '/') && glob[s.glob] == '/' {
						if s.path == 0 || (s.path < len(path) && path[s.path-1] == '/') {
							s.glob++
						}
						s.globstar = s.wildcard
					}
				} else {
					s.glob++
				}

				if s.globstar.path != s.wildcard.path && s.path < len(path) && path[s.path] == '/' {
					if s.globstar.path > 0 && s.path+1 < len(path) {
						s.glob = s.globstar.glob
						s.wildcard.glob = s.globstar.glob
					} else {
						s.wildcard.path = 0
					}
				}

				if braces.length > 0 && s.glob < len(glob) && (glob[s.glob] == ',' || glob[s.glob] == '}') {
					if s.skipBraces(glob, false) == braceInvalid {
						return false
					}
				}
				continue

			case c == '?' && s.path < len(path):
				if path[s.path] != '/' {
					s.glob++
					s.path++
					continue
				}

			case c == '[' && s.path < len(path):
				s.glob++
				pc := path[s.path]
				classNegated := false
				if s.glob < len(glob) && (glob[s.glob] == '^' || glob[s.glob] == '!') {
					classNegated = true
					s.glob++
				}
				first := true
				isMatch := false
				for s.glob < len(glob) && (first || glob[s.glob] != ']') {
					low, ok := unescape(glob[s.glob], glob, &s.glob)
					if !ok {
						return false
					}
					s.glob++
					high := low
					if s.glob+1 < len(glob) && glob[s.glob] == '-' && glob[s.glob+1] != ']' {
						s.glob++
						high, ok = unescape(glob[s.glob], glob, &s.glob)
						if !ok {
							return false
						}
						s.glob++
					}
					if low <= pc && pc <= high {
						isMatch = true
					}
					first = false
				}
				if s.glob >= len(glob) {
					return false
				}
				s.glob++
				if isMatch != classNegated {
					s.path++
					continue
				}

			case c == '{' && s.path < len(path):
				if braces.length >= len(braces.stack) {
					return false
				}
				s = braces.push(s)
				continue

			case c == '}' && braces.length > 0:
				braces.longest = max(braces.longest, s.path)
				s.glob++
				s = braces.pop(s)
				continue

			case c == ',' && braces.length > 0:
				braces.longest = max(braces.longest, s.path)
				s.path = braces.last().path
				s.glob++
				s.wildcard = wildcard{}
				s.globstar = wildcard{}
				continue

			case s.path < len(path):
				lit, ok := unescape(c, glob, &s.glob)
				if !ok {
					return false
				}
				if path[s.path] == lit {
					if braces.length > 0 && s.glob > 0 && glob[s.glob-1] == '}' {
						braces.longest = s.path
						s = braces.pop(s)
					}
					s.glob++
					s.path++
					if lit != '/' {
						s.globstar.path = 0
					}
					continue
				}
			}
		}

		if s.wildcard.path > 0 && s.wildcard.path <= len(path) {
			s.backtrack()
			continue
		}

		if braces.length > 0 {
			switch s.skipBraces(glob, true) {
			case braceInvalid:
				return false
			case braceComma:
				s.path = braces.last().path
				continue
			}
			if braces.longest > 0 {
				s = braces.pop(s)
				continue
			}
			s = *braces.last()
			braces.length--
			if s.wildcard.path > 0 && s.wildcard.path <= len(path) {
				s.backtrack()
				continue
			}
		}

		return negated
	}

	if braces.length > 0 && s.glob > 0 && glob[s.glob-1] == '}' {
		braces.longest = s.path
		braces.pop(s)
	}
	return !negated
}

type wildcard struct{ glob, path int }

type globState struct {
	path, glob         int
	wildcard, globstar wildcard
}

func (s *globState) backtrack() {
	s.glob = s.wildcard.glob
	s.path = s.wildcard.path
}

type braceResult int

const (
	braceInvalid braceResult = iota
	braceComma
	braceEnd
)

func (s *globState) skipBraces(glob string, stopOnComma bool) braceResult {
	depth := 1
	inBrackets := false
	for s.glob < len(glob) && depth > 0 {
		switch c := glob[s.glob]; {
		case c == '{' && !inBrackets:
			depth++
		case c == '}' && !inBrackets:
			depth--
		case c == ',' && stopOnComma && depth == 1 && !inBrackets:
			s.glob++
			return braceComma
		case (c == '*' || c == '?' || c == '[') && !inBrackets:
			if c == '[' {
				inBrackets = true
			}
			if c == '*' && s.glob+1 < len(glob) && glob[s.glob+1] == '*' {
				s.glob = skipGlobstars(glob, s.glob+2) - 2
				s.glob++
			}
		case c == ']':
			inBrackets = false
		case c == '\\':
			s.glob++
		}
		s.glob++
	}
	if depth != 0 {
		return braceInvalid
	}
	return braceEnd
}

func unescape(c byte, glob string, i *int) (byte, bool) {
	if c != '\\' {
		return c, true
	}
	*i++
	if *i >= len(glob) {
		return 0, false
	}
	switch glob[*i] {
	case 'a':
		return 'a', true
	case 'b':
		return '\b', true
	case 'n':
		return '\n', true
	case 'r':
		return '\r', true
	case 't':
		return '\t', true
	default:
		return glob[*i], true
	}
}

func skipGlobstars(glob string, i int) int {
	for i+3 <= len(glob) && glob[i:i+3] == "/**" {
		i += 3
	}
	return i
}

type braceStack struct {
	stack   [10]globState
	length  int
	longest int
}

func (b *braceStack) push(s globState) globState {
	b.stack[b.length] = s
	b.length++
	return globState{path: s.path, glob: s.glob + 1}
}

func (b *braceStack) pop(s globState) globState {
	b.length--
	out := globState{
		path:     b.longest,
		glob:     s.glob,
		wildcard: b.stack[b.length].wildcard,
		globstar: b.stack[b.length].globstar,
	}
	if b.length == 0 {
		b.longest = 0
	}
	return out
}

func (b *braceStack) last() *globState { return &b.stack[b.length-1] }
