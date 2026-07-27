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
	"reflect"
	"strings"
	"testing"

	common "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_common"
)

// ---------------------------------------------------------------------------
// path parser
// ---------------------------------------------------------------------------

func TestParsePathValid(t *testing.T) {
	cases := []struct {
		path      string
		allowQ    bool
		wantSegs  string // rendered form
		wantQuery string // "wire=field,wire=field" or ""
	}{
		{"/counters/{name}", false, "counters,{name}", ""},
		{"/", false, "", ""},
		{"/files/{*rest}", false, "files,{*rest}", ""},
		{"/{agent-type}/{name}", false, "{sys:agent-type},{name}", ""},
		{"/{agent-version}", false, "{sys:agent-version}", ""},
		{"/add?by={by}", true, "add", "by=by"},
		{"/search?q={query}&n={limit}", true, "search", "q=query,n=limit"},
	}
	for _, c := range cases {
		pp, errs := parsePath(c.path, c.allowQ)
		if len(errs) != 0 {
			t.Errorf("%q: unexpected errors %v", c.path, errs)
			continue
		}
		if got := renderSegs2(pp.segs); got != c.wantSegs {
			t.Errorf("%q: segs = %q, want %q", c.path, got, c.wantSegs)
		}
		if got := renderQuery(pp.query); got != c.wantQuery {
			t.Errorf("%q: query = %q, want %q", c.path, got, c.wantQuery)
		}
	}
}

func TestParsePathErrors(t *testing.T) {
	cases := []struct {
		path   string
		allowQ bool
		want   string
	}{
		{"counters/{name}", false, "must start with '/'"},
		{"/a//b", false, "empty path segment"},
		{"/a/ b", false, "must not contain whitespace"},
		{"/a{b}", false, "whole-segment"},
		{"/{*rest}/more", false, "must be the last"},
		{"/x?y={z}", false, "query parameters are not allowed"},
		{"/x?bad", true, "must be of the form name={field}"},
		{"/x?a=notref", true, "must be a {field} reference"},
		{"/x?={y}", true, "empty parameter name"},
		{"", false, "must not be empty"},
		{"/{a-b}", false, "invalid character"},
		{"/{*}", false, "must not be empty"},
		{"/{}", false, "empty {} variable"},
	}
	for _, c := range cases {
		_, errs := parsePath(c.path, c.allowQ)
		if !containsSubstr(errs, c.want) {
			t.Errorf("%q: errors %v, want one mentioning %q", c.path, errs, c.want)
		}
	}
}

// ---------------------------------------------------------------------------
// buildHTTP: full mapping + every validation
// ---------------------------------------------------------------------------

func TestBuildHTTPCompilesAFullMountAndEndpoints(t *testing.T) {
	e := agent("CounterAgent",
		&Mount{Path: "/counters/{name}", Auth: true, CORS: []string{"*"}, PhantomAgent: true, WebhookSuffix: "/events"},
		fields("name"),
		method("add", fields("by", "tenant"),
			POST("/add?by={by}", Header("X-Tenant", "tenant"), EndpointAuth(false), EndpointCORS("https://x")),
			GET("/add/{by}", Query("t", "tenant")),
		),
		method("value", nil, GET("/value")),
	)

	mount, endpoints, errs := buildHTTP(e)
	if len(errs) != 0 {
		t.Fatalf("unexpected definition errors: %v", errs)
	}
	if !mount.IsSome() {
		t.Fatal("expected a mount")
	}
	m := mount.Some()
	if got := renderSegs(m.PathPrefix); got != "/counters/{name}" {
		t.Errorf("mount prefix = %q", got)
	}
	if !m.AuthDetails.IsSome() || !m.AuthDetails.Some().Required {
		t.Errorf("mount auth not required")
	}
	if !m.PhantomAgent {
		t.Errorf("phantom agent not set")
	}
	if got := renderSegs(m.WebhookSuffix); got != "/events" {
		t.Errorf("webhook suffix = %q", got)
	}
	if len(m.CorsOptions.AllowedPatterns) != 1 || m.CorsOptions.AllowedPatterns[0] != "*" {
		t.Errorf("mount cors = %v", m.CorsOptions.AllowedPatterns)
	}

	add := endpoints["add"]
	if len(add) != 2 {
		t.Fatalf("add: want 2 endpoints, got %d", len(add))
	}
	// endpoint 0: POST /add?by={by} with X-Tenant header, auth override false
	if add[0].HttpMethod.Tag() != common.HttpMethodPost {
		t.Errorf("add[0] method tag = %d", add[0].HttpMethod.Tag())
	}
	if got := renderSegs(add[0].PathSuffix); got != "/add" {
		t.Errorf("add[0] suffix = %q", got)
	}
	if len(add[0].QueryVars) != 1 || add[0].QueryVars[0].QueryParamName != "by" || add[0].QueryVars[0].VariableName != "by" {
		t.Errorf("add[0] query = %v", add[0].QueryVars)
	}
	if len(add[0].HeaderVars) != 1 || add[0].HeaderVars[0].HeaderName != "X-Tenant" || add[0].HeaderVars[0].VariableName != "tenant" {
		t.Errorf("add[0] headers = %v", add[0].HeaderVars)
	}
	if !add[0].AuthDetails.IsSome() || add[0].AuthDetails.Some().Required {
		t.Errorf("add[0] auth override should be Some(false)")
	}
	// endpoint 1: GET /add/{by}?t={tenant} — bodyless, binds by (path) + tenant (query)
	if got := renderSegs(add[1].PathSuffix); got != "/add/{by}" {
		t.Errorf("add[1] suffix = %q", got)
	}
	if len(add[1].QueryVars) != 1 || add[1].QueryVars[0].QueryParamName != "t" {
		t.Errorf("add[1] query = %v", add[1].QueryVars)
	}
	if add[1].AuthDetails.IsSome() {
		t.Errorf("add[1] auth should inherit (None)")
	}
}

func TestBuildHTTPCustomVerb(t *testing.T) {
	e := agent("A", &Mount{Path: "/a/{id}"}, fields("id"),
		method("report", nil, Custom("REPORT", "/report")))
	_, endpoints, errs := buildHTTP(e)
	if len(errs) != 0 {
		t.Fatalf("errs: %v", errs)
	}
	m := endpoints["report"][0].HttpMethod
	if m.Tag() != common.HttpMethodCustom || m.Custom() != "REPORT" {
		t.Errorf("verb = tag %d %q", m.Tag(), m.Custom())
	}
}

func TestVerbConstructorsMapToWit(t *testing.T) {
	cases := []struct {
		ep  Endpoint
		tag uint8
	}{
		{GET("/x"), common.HttpMethodGet},
		{HEAD("/x"), common.HttpMethodHead},
		{POST("/x"), common.HttpMethodPost},
		{PUT("/x"), common.HttpMethodPut},
		{DELETE("/x"), common.HttpMethodDelete},
		{CONNECT("/x"), common.HttpMethodConnect},
		{OPTIONS("/x"), common.HttpMethodOptions},
		{TRACE("/x"), common.HttpMethodTrace},
		{PATCH("/x"), common.HttpMethodPatch},
	}
	for _, c := range cases {
		if got := witMethod(c.ep.method).Tag(); got != c.tag {
			t.Errorf("%s -> tag %d, want %d", c.ep.method, got, c.tag)
		}
	}
}

// System-variable and catch-all segments compile on both the mount prefix and an
// endpoint suffix.
func TestBuildHTTPSystemVarAndCatchAllSegments(t *testing.T) {
	e := agent("A", &Mount{Path: "/{agent-type}/{id}"}, fields("id"),
		method("m", fields("rest"), GET("/files/{*rest}")))
	mount, endpoints, errs := buildHTTP(e)
	if len(errs) != 0 {
		t.Fatalf("errs: %v", errs)
	}
	if got := renderSegs(mount.Some().PathPrefix); got != "/{agent-type}/{id}" {
		t.Errorf("prefix = %q", got)
	}
	if got := renderSegs(endpoints["m"][0].PathSuffix); got != "/files/{*rest}" {
		t.Errorf("suffix = %q", got)
	}
}

func TestBuildHTTPValidations(t *testing.T) {
	cases := []struct {
		name string
		e    *agentEntry
		want string
	}{
		{
			"mount var not an id field",
			agent("A", &Mount{Path: "/a/{tenant}"}, fields("id")),
			"{tenant} is not a constructor",
		},
		{
			"mount does not cover id",
			agent("A", &Mount{Path: "/a/{id}"}, fields("id", "region")),
			`does not bind Id field "region"`,
		},
		{
			"endpoints without a mount",
			agent("A", nil, fields("id"), method("m", nil, GET("/m"))),
			"no HTTP mount",
		},
		{
			"endpoint var not an input field",
			agent("A", &Mount{Path: "/a/{id}"}, fields("id"), method("m", fields("x"), POST("/m/{y}"))),
			"{y} is not an input field",
		},
		{
			"bodyless must bind every field",
			agent("A", &Mount{Path: "/a/{id}"}, fields("id"), method("m", fields("page", "size"), GET("/m?p={page}"))),
			`"size" is unbound`,
		},
		{
			"duplicate binding",
			agent("A", &Mount{Path: "/a/{id}"}, fields("id"), method("m", fields("x"), POST("/m/{x}?x={x}"))),
			`"x" is bound 2 times`,
		},
		{
			"non-scalar field not bindable",
			agent("A", &Mount{Path: "/a/{id}"}, fields("id"),
				method("m", []fieldInfo{{name: "blob", typ: reflect.TypeFor[[]byte]()}}, GET("/m/{blob}"))),
			"only scalar fields",
		},
		{
			"webhook suffix must be literal",
			agent("A", &Mount{Path: "/a/{id}", WebhookSuffix: "/{oops}"}, fields("id")),
			"only literal segments",
		},
		{
			"duplicate query wire name",
			agent("A", &Mount{Path: "/a/{id}"}, fields("id"), method("m", fields("x", "y"), POST("/m?k={x}", Query("k", "y")))),
			`duplicate query parameter "k"`,
		},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			_, _, errs := buildHTTP(c.e)
			if !containsDefErr(errs, c.want) {
				t.Errorf("errors %v, want one mentioning %q", errs, c.want)
			}
		})
	}
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

func fields(names ...string) []fieldInfo {
	out := make([]fieldInfo, len(names))
	for i, n := range names {
		out[i] = fieldInfo{name: n, typ: reflect.TypeFor[string]()}
	}
	return out
}

func method(name string, in []fieldInfo, eps ...Endpoint) *methodEntry {
	return &methodEntry{name: name, inFields: in, endpoints: eps}
}

func agent(name string, mount *Mount, idFields []fieldInfo, methods ...*methodEntry) *agentEntry {
	e := &agentEntry{name: name, mount: mount, idFields: idFields, methods: map[string]*methodEntry{}}
	for _, m := range methods {
		e.methods[m.name] = m
		e.order = append(e.order, m.name)
	}
	return e
}

func renderSegs(segs []common.PathSegment) string {
	var parts []string
	for _, s := range segs {
		switch s.Tag() {
		case common.PathSegmentLiteral:
			parts = append(parts, s.Literal())
		case common.PathSegmentPathVariable:
			parts = append(parts, "{"+s.PathVariable().VariableName+"}")
		case common.PathSegmentRemainingPathVariable:
			parts = append(parts, "{*"+s.RemainingPathVariable().VariableName+"}")
		case common.PathSegmentSystemVariable:
			if s.SystemVariable() == common.SystemVariableAgentVersion {
				parts = append(parts, "{agent-version}")
			} else {
				parts = append(parts, "{agent-type}")
			}
		}
	}
	return "/" + strings.Join(parts, "/")
}

// renderSegs2 renders parsed (pre-WIT) segments for parser tests.
func renderSegs2(segs []pathSeg) string {
	var parts []string
	for _, s := range segs {
		switch s.kind {
		case literalSeg:
			parts = append(parts, s.value)
		case varSeg:
			parts = append(parts, "{"+s.value+"}")
		case restSeg:
			parts = append(parts, "{*"+s.value+"}")
		case sysSeg:
			parts = append(parts, "{sys:"+s.value+"}")
		}
	}
	return strings.Join(parts, ",")
}

func renderQuery(q []wireBind) string {
	var parts []string
	for _, b := range q {
		parts = append(parts, b.wire+"="+b.field)
	}
	return strings.Join(parts, ",")
}

func containsSubstr(errs []string, want string) bool {
	for _, e := range errs {
		if strings.Contains(e, want) {
			return true
		}
	}
	return false
}

func containsDefErr(errs []definitionError, want string) bool {
	for _, e := range errs {
		if strings.Contains(e.Error(), want) {
			return true
		}
	}
	return false
}
