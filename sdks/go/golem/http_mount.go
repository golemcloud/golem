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
	"sort"
	"strings"
	"unicode"

	common "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_common"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// HTTP mounting is metadata only. The guest publishes an agent-level mount and
// per-method endpoints; the platform compiles those into routes and, when a
// request arrives, derives the agent id from the mount path variables and the
// method arguments from the endpoint's path/query/header bindings (and body),
// then calls the ordinary initialize/invoke exports. The guest never handles a
// raw HTTP request — there is no incoming-handler to implement here.
//
// Route strings are not checked at compile time, so every binding rule is
// validated at registration and reported at discovery rather than surfacing as a
// runtime surprise.

// Mount declares that an agent's methods are reachable over HTTP under a common
// path prefix. Set it on [Spec.HTTP].
//
// The prefix's {var} segments bind the agent's constructor (Id) fields, so the
// platform can tell which instance a request addresses — every Id field must
// appear as a {var}. Segments may also be literals, the catch-all {*rest} (last
// only), or the system tokens {agent-type} / {agent-version}.
type Mount struct {
	// Path is the mount prefix, e.g. "/counters/{name}". Required.
	Path string
	// Auth requires authentication on every endpoint (an endpoint may override).
	Auth bool
	// CORS is the list of allowed-origin patterns; empty means none.
	CORS []string
	// PhantomAgent provisions a fresh instance per request instead of routing to
	// a persistent one.
	PhantomAgent bool
	// WebhookSuffix is an optional, literal-only path suffix advertised for
	// webhooks.
	WebhookSuffix string
}

// Endpoint is one HTTP route for a method: a verb plus a path suffix (appended
// to the agent's mount prefix) and how request data binds to the method's input
// fields. Build one with a verb constructor ([GET], [POST], …) and configure it
// with [EndpointOpt]s; a method may have several via [HTTP].
type Endpoint struct {
	method    string
	path      string
	headers   []wireBind // wire header name -> input field
	query     []wireBind // wire query name -> input field (in addition to inline ?k={f})
	auth      *bool      // nil = inherit the mount; non-nil = override
	authCount int        // how many times EndpointAuth was applied (>1 is a misuse)
	cors      []string
}

// wireBind maps a wire name (a header or query-parameter name) to the input
// field it fills.
type wireBind struct{ wire, field string }

// EndpointOpt configures an [Endpoint].
type EndpointOpt func(*Endpoint)

func newEndpoint(method, path string, opts []EndpointOpt) Endpoint {
	e := Endpoint{method: method, path: path}
	for _, o := range opts {
		o(&e)
	}
	return e
}

// GET builds a GET endpoint; like the other verb constructors below it names an
// HTTP method plus a path suffix. GET and HEAD are bodyless (they must bind
// every input field, since there is no request body to carry the rest); the
// other verbs carry unbound fields in the body.
func GET(path string, opts ...EndpointOpt) Endpoint     { return newEndpoint("GET", path, opts) }
func HEAD(path string, opts ...EndpointOpt) Endpoint    { return newEndpoint("HEAD", path, opts) }
func POST(path string, opts ...EndpointOpt) Endpoint    { return newEndpoint("POST", path, opts) }
func PUT(path string, opts ...EndpointOpt) Endpoint     { return newEndpoint("PUT", path, opts) }
func DELETE(path string, opts ...EndpointOpt) Endpoint  { return newEndpoint("DELETE", path, opts) }
func CONNECT(path string, opts ...EndpointOpt) Endpoint { return newEndpoint("CONNECT", path, opts) }
func OPTIONS(path string, opts ...EndpointOpt) Endpoint { return newEndpoint("OPTIONS", path, opts) }
func TRACE(path string, opts ...EndpointOpt) Endpoint   { return newEndpoint("TRACE", path, opts) }
func PATCH(path string, opts ...EndpointOpt) Endpoint   { return newEndpoint("PATCH", path, opts) }

// Custom builds an endpoint with a non-standard HTTP verb.
func Custom(verb, path string, opts ...EndpointOpt) Endpoint { return newEndpoint(verb, path, opts) }

// Header binds an incoming request header to an input field. wire is the header
// name on the wire (e.g. "X-Tenant"); field is the method input field it fills.
func Header(wire, field string) EndpointOpt {
	return func(e *Endpoint) { e.headers = append(e.headers, wireBind{wire, field}) }
}

// Query binds a query parameter to an input field, for when the parameter name
// differs from the field name (inline "?name={field}" in the path is equivalent
// when they match).
func Query(wire, field string) EndpointOpt {
	return func(e *Endpoint) { e.query = append(e.query, wireBind{wire, field}) }
}

// EndpointAuth overrides the mount's auth requirement for this endpoint. Setting
// it more than once on one endpoint is a definition error, not a silent
// overwrite.
func EndpointAuth(required bool) EndpointOpt {
	return func(e *Endpoint) { e.auth = &required; e.authCount++ }
}

// EndpointCORS sets allowed-origin patterns for this endpoint.
func EndpointCORS(patterns ...string) EndpointOpt {
	return func(e *Endpoint) { e.cors = append(e.cors, patterns...) }
}

// ---------------------------------------------------------------------------
// Path parsing (pure)
// ---------------------------------------------------------------------------

type segKind int

const (
	literalSeg segKind = iota
	varSeg
	restSeg
	sysSeg
)

type pathSeg struct {
	kind  segKind
	value string // literal text, variable name, or system-token name
}

type parsedPath struct {
	segs  []pathSeg
	query []wireBind
}

// parsePath parses a path string into segments (and, when allowQuery, an inline
// query string). It returns every problem it finds rather than stopping at the
// first, so a bad route surfaces all its issues at once.
func parsePath(path string, allowQuery bool) (parsedPath, []string) {
	var out parsedPath
	var errs []string
	if path == "" {
		return out, []string{"path must not be empty"}
	}

	rawPath := path
	if i := strings.IndexByte(path, '?'); i >= 0 {
		rawPath = path[:i]
		if !allowQuery {
			errs = append(errs, "query parameters are not allowed in a mount or webhook path")
		} else {
			out.query, errs = parseQuery(path[i+1:], errs)
		}
	}

	if !strings.HasPrefix(rawPath, "/") {
		return out, append(errs, "path must start with '/'")
	}
	body := rawPath[1:]
	if body == "" {
		return out, errs // just "/": no segments
	}

	for _, p := range strings.Split(body, "/") {
		seg, serrs := parseSegment(p)
		errs = append(errs, serrs...)
		if seg != nil {
			out.segs = append(out.segs, *seg)
		}
	}
	for i, s := range out.segs {
		if s.kind == restSeg && i != len(out.segs)-1 {
			errs = append(errs, fmt.Sprintf("catch-all {*%s} must be the last path segment", s.value))
		}
	}
	return out, errs
}

func parseSegment(p string) (*pathSeg, []string) {
	switch {
	case p == "":
		return nil, []string{"empty path segment (check for '//' or a trailing '/')"}
	case strings.ContainsAny(p, " \t"):
		return nil, []string{fmt.Sprintf("path segment %q must not contain whitespace", p)}
	case !strings.ContainsAny(p, "{}"):
		return &pathSeg{kind: literalSeg, value: p}, nil
	}
	if !strings.HasPrefix(p, "{") || !strings.HasSuffix(p, "}") ||
		strings.Count(p, "{") != 1 || strings.Count(p, "}") != 1 {
		return nil, []string{fmt.Sprintf("path segment %q must be either a literal or a whole-segment {variable}", p)}
	}
	inner := p[1 : len(p)-1]
	switch {
	case inner == "":
		return nil, []string{"empty {} variable"}
	case inner == "agent-type" || inner == "agent-version":
		return &pathSeg{kind: sysSeg, value: inner}, nil
	case strings.HasPrefix(inner, "*"):
		name := inner[1:]
		if e := validVarName(name); e != "" {
			return nil, []string{"catch-all " + e}
		}
		return &pathSeg{kind: restSeg, value: name}, nil
	default:
		if e := validVarName(inner); e != "" {
			return nil, []string{e}
		}
		return &pathSeg{kind: varSeg, value: inner}, nil
	}
}

func parseQuery(qs string, errs []string) ([]wireBind, []string) {
	var out []wireBind
	if qs == "" {
		return out, append(errs, "empty query string after '?'")
	}
	for _, pair := range strings.Split(qs, "&") {
		eq := strings.IndexByte(pair, '=')
		if eq < 0 {
			errs = append(errs, fmt.Sprintf("query %q must be of the form name={field}", pair))
			continue
		}
		wire, val := pair[:eq], pair[eq+1:]
		if wire == "" {
			errs = append(errs, fmt.Sprintf("query %q has an empty parameter name", pair))
			continue
		}
		if !strings.HasPrefix(val, "{") || !strings.HasSuffix(val, "}") {
			errs = append(errs, fmt.Sprintf("query parameter %q value must be a {field} reference", wire))
			continue
		}
		field := val[1 : len(val)-1]
		if e := validVarName(field); e != "" {
			errs = append(errs, e)
			continue
		}
		out = append(out, wireBind{wire: wire, field: field})
	}
	return out, errs
}

func validVarName(n string) string {
	if n == "" {
		return "variable name must not be empty"
	}
	for _, r := range n {
		if r != '_' && !unicode.IsLetter(r) && !unicode.IsDigit(r) {
			return fmt.Sprintf("variable name %q has an invalid character %q", n, string(r))
		}
	}
	return ""
}

func isBodyless(verb string) bool {
	switch strings.ToUpper(verb) {
	case "GET", "HEAD":
		return true
	}
	return false
}

// bindableKind reports whether a field of this kind can be filled from a URL
// path/query/header value — only scalars are string-decodable.
func bindableKind(k reflect.Kind) bool {
	switch k {
	case reflect.String, reflect.Bool,
		reflect.Int, reflect.Int8, reflect.Int16, reflect.Int32, reflect.Int64,
		reflect.Uint, reflect.Uint8, reflect.Uint16, reflect.Uint32, reflect.Uint64,
		reflect.Float32, reflect.Float64:
		return true
	}
	return false
}

// ---------------------------------------------------------------------------
// Validate + compile to WIT
// ---------------------------------------------------------------------------

// buildHTTP validates an agent's mount and endpoints and compiles them to the
// WIT metadata reported at discovery. Every problem is returned as an attributed
// definition error rather than panicking; the compiled records are still
// returned so a well-formed part of a partly-broken component still publishes
// (discovery gates on the errors, so nothing invalid actually deploys).
func buildHTTP(e *agentEntry) (witTypes.Option[common.HttpMountDetails], map[string][]common.HttpEndpointDetails, []definitionError) {
	var errs []definitionError
	rec := func(method, format string, args ...any) {
		errs = append(errs, definitionError{agent: e.name, method: method, detail: fmt.Sprintf(format, args...)})
	}

	if e.mount == nil {
		for _, name := range e.order {
			if len(e.methods[name].endpoints) > 0 {
				rec(name, "declares HTTP endpoint(s) but the agent has no HTTP mount; set Spec.HTTP")
			}
		}
		return witTypes.None[common.HttpMountDetails](), nil, errs
	}

	mp, perrs := parsePath(e.mount.Path, false)
	for _, pe := range perrs {
		rec("", "HTTP mount path %q: %s", e.mount.Path, pe)
	}

	idNames := fieldNameSet(e.idFields)
	mountVars := map[string]bool{}
	for _, s := range mp.segs {
		if s.kind == varSeg || s.kind == restSeg {
			mountVars[s.value] = true
			if !idNames[s.value] {
				rec("", "HTTP mount path variable {%s} is not a constructor (Id) field", s.value)
			}
		}
	}
	for _, f := range e.idFields {
		if !mountVars[f.name] {
			rec("", "HTTP mount path does not bind Id field %q; every constructor field must appear as a {var}", f.name)
		}
	}

	webhook := []common.PathSegment{}
	if e.mount.WebhookSuffix != "" {
		wp, werrs := parsePath(e.mount.WebhookSuffix, false)
		for _, we := range werrs {
			rec("", "HTTP webhook suffix %q: %s", e.mount.WebhookSuffix, we)
		}
		for _, s := range wp.segs {
			if s.kind != literalSeg {
				rec("", "HTTP webhook suffix must contain only literal segments, got {%s}", s.value)
			}
		}
		webhook = witSegments(wp.segs)
	}

	mount := common.HttpMountDetails{
		PathPrefix:    witSegments(mp.segs),
		AuthDetails:   witTypes.Some(common.AuthDetails{Required: e.mount.Auth}),
		PhantomAgent:  e.mount.PhantomAgent,
		CorsOptions:   common.CorsOptions{AllowedPatterns: e.mount.CORS},
		WebhookSuffix: webhook,
	}

	endpoints := map[string][]common.HttpEndpointDetails{}
	for _, name := range e.order {
		m := e.methods[name]
		inNames := fieldNameSet(m.inFields)
		inKind := fieldKindMap(m.inFields)
		for _, ep := range m.endpoints {
			det, eerrs := validateAndCompileEndpoint(ep, inNames, inKind, mountVars)
			for _, ee := range eerrs {
				rec(name, "%s", ee)
			}
			endpoints[name] = append(endpoints[name], det)
		}
	}

	return witTypes.Some(mount), endpoints, errs
}

func validateAndCompileEndpoint(ep Endpoint, inNames map[string]bool, inKind map[string]reflect.Kind, mountVars map[string]bool) (common.HttpEndpointDetails, []string) {
	var errs []string
	if ep.authCount > 1 {
		errs = append(errs, fmt.Sprintf("%s %q: EndpointAuth set %d times (an endpoint has one auth setting)", ep.method, ep.path, ep.authCount))
	}
	pp, perrs := parsePath(ep.path, true)
	for _, pe := range perrs {
		errs = append(errs, fmt.Sprintf("%s %q: %s", ep.method, ep.path, pe))
	}

	bound := map[string]int{}
	bind := func(field, where string) {
		if !inNames[field] && !mountVars[field] {
			errs = append(errs, fmt.Sprintf("%s variable {%s} is not an input field of the method (nor a mount variable)", where, field))
			return
		}
		if inNames[field] {
			if k, ok := inKind[field]; ok && !bindableKind(k) {
				errs = append(errs, fmt.Sprintf("%s cannot bind input field %q (kind %s); only scalar fields decode from a URL", where, field, k))
			}
		}
		bound[field]++
	}

	for _, s := range pp.segs {
		if s.kind == varSeg || s.kind == restSeg {
			bind(s.value, "path")
		}
	}
	queryWire := map[string]bool{}
	addQuery := func(qs []wireBind) {
		for _, q := range qs {
			if queryWire[q.wire] {
				errs = append(errs, fmt.Sprintf("duplicate query parameter %q", q.wire))
			}
			queryWire[q.wire] = true
			bind(q.field, "query")
		}
	}
	addQuery(pp.query)
	addQuery(ep.query)

	headerWire := map[string]bool{}
	for _, h := range ep.headers {
		lw := strings.ToLower(h.wire)
		if headerWire[lw] {
			errs = append(errs, fmt.Sprintf("duplicate header %q", h.wire))
		}
		headerWire[lw] = true
		bind(h.field, "header")
	}

	for _, f := range sortedKeys(bound) {
		if bound[f] > 1 {
			errs = append(errs, fmt.Sprintf("input field %q is bound %d times; bind it from exactly one of path/query/header", f, bound[f]))
		}
	}

	if isBodyless(ep.method) {
		for _, f := range sortedKeys(inNames) {
			if bound[f] == 0 {
				errs = append(errs, fmt.Sprintf("%s endpoint must bind every input field, but %q is unbound (a bodyless verb carries no request body)", ep.method, f))
			}
		}
	}

	queryVars := make([]common.QueryVariable, 0, len(pp.query)+len(ep.query))
	for _, q := range pp.query {
		queryVars = append(queryVars, common.QueryVariable{QueryParamName: q.wire, VariableName: q.field})
	}
	for _, q := range ep.query {
		queryVars = append(queryVars, common.QueryVariable{QueryParamName: q.wire, VariableName: q.field})
	}
	headerVars := make([]common.HeaderVariable, 0, len(ep.headers))
	for _, h := range ep.headers {
		headerVars = append(headerVars, common.HeaderVariable{HeaderName: h.wire, VariableName: h.field})
	}
	auth := witTypes.None[common.AuthDetails]()
	if ep.auth != nil {
		auth = witTypes.Some(common.AuthDetails{Required: *ep.auth})
	}

	det := common.HttpEndpointDetails{
		HttpMethod:  witMethod(ep.method),
		PathSuffix:  witSegments(pp.segs),
		HeaderVars:  headerVars,
		QueryVars:   queryVars,
		AuthDetails: auth,
		CorsOptions: common.CorsOptions{AllowedPatterns: ep.cors},
	}
	return det, errs
}

func witSegments(segs []pathSeg) []common.PathSegment {
	out := make([]common.PathSegment, 0, len(segs))
	for _, s := range segs {
		switch s.kind {
		case literalSeg:
			out = append(out, common.MakePathSegmentLiteral(s.value))
		case varSeg:
			out = append(out, common.MakePathSegmentPathVariable(common.PathVariable{VariableName: s.value}))
		case restSeg:
			out = append(out, common.MakePathSegmentRemainingPathVariable(common.PathVariable{VariableName: s.value}))
		case sysSeg:
			sv := common.SystemVariableAgentType
			if s.value == "agent-version" {
				sv = common.SystemVariableAgentVersion
			}
			out = append(out, common.MakePathSegmentSystemVariable(sv))
		}
	}
	return out
}

func witMethod(verb string) common.HttpMethod {
	switch strings.ToUpper(verb) {
	case "GET":
		return common.MakeHttpMethodGet()
	case "HEAD":
		return common.MakeHttpMethodHead()
	case "POST":
		return common.MakeHttpMethodPost()
	case "PUT":
		return common.MakeHttpMethodPut()
	case "DELETE":
		return common.MakeHttpMethodDelete()
	case "CONNECT":
		return common.MakeHttpMethodConnect()
	case "OPTIONS":
		return common.MakeHttpMethodOptions()
	case "TRACE":
		return common.MakeHttpMethodTrace()
	case "PATCH":
		return common.MakeHttpMethodPatch()
	default:
		return common.MakeHttpMethodCustom(verb)
	}
}

// routeKey renders a verb plus the full path (mount prefix ++ endpoint suffix)
// into a normalized string for collision detection. Variable names are erased,
// because the gateway matches on position — so GET /a/{x} and GET /a/{y} are the
// same route and must not both be declared.
func routeKey(method common.HttpMethod, prefix, suffix []common.PathSegment) string {
	var b strings.Builder
	b.WriteString(verbName(method))
	for _, group := range [][]common.PathSegment{prefix, suffix} {
		for _, s := range group {
			b.WriteByte('/')
			switch s.Tag() {
			case common.PathSegmentLiteral:
				b.WriteString(s.Literal())
			case common.PathSegmentPathVariable:
				b.WriteString("{}")
			case common.PathSegmentRemainingPathVariable:
				b.WriteString("{*}")
			case common.PathSegmentSystemVariable:
				if s.SystemVariable() == common.SystemVariableAgentVersion {
					b.WriteString("{agent-version}")
				} else {
					b.WriteString("{agent-type}")
				}
			}
		}
	}
	return b.String()
}

func verbName(m common.HttpMethod) string {
	switch m.Tag() {
	case common.HttpMethodGet:
		return "GET"
	case common.HttpMethodHead:
		return "HEAD"
	case common.HttpMethodPost:
		return "POST"
	case common.HttpMethodPut:
		return "PUT"
	case common.HttpMethodDelete:
		return "DELETE"
	case common.HttpMethodConnect:
		return "CONNECT"
	case common.HttpMethodOptions:
		return "OPTIONS"
	case common.HttpMethodTrace:
		return "TRACE"
	case common.HttpMethodPatch:
		return "PATCH"
	default:
		return m.Custom()
	}
}

func fieldNameSet(fs []fieldInfo) map[string]bool {
	m := make(map[string]bool, len(fs))
	for _, f := range fs {
		m[f.name] = true
	}
	return m
}

func fieldKindMap(fs []fieldInfo) map[string]reflect.Kind {
	m := make(map[string]reflect.Kind, len(fs))
	for _, f := range fs {
		m[f.name] = f.typ.Kind()
	}
	return m
}

func sortedKeys[V any](m map[string]V) []string {
	out := make([]string, 0, len(m))
	for k := range m {
		out = append(out, k)
	}
	sort.Strings(out)
	return out
}
