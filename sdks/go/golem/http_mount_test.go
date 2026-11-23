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
	"github.com/golemcloud/golem/sdks/go/golem/internal/engine"
	"reflect"
	"strings"
	"testing"

	common "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_common"
	coreHTTP "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_http"
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
	if add[0].HttpMethod.Tag() != common.HttpMethodMethod || add[0].HttpMethod.Method().Tag() != coreHTTP.HttpMethodPost {
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

func TestBuildHTTPImplicitQueries(t *testing.T) {
	for _, endpoint := range []Endpoint{GET("/m?p={page}"), HEAD("/m?p={page}"), DELETE("/m?p={page}")} {
		t.Run(endpoint.method, func(t *testing.T) {
			inputs := append(fields("page", "size"),
				engine.Field{Name: "tags", Typ: reflect.TypeFor[[]string]()},
				engine.Field{Name: "bytes", Typ: reflect.TypeFor[[]uint8]()},
				engine.Field{Name: "labels", Typ: reflect.TypeFor[Option[map[string]int32]]()},
			)
			e := agent("A", &Mount{Path: "/a/{id}"}, fields("id"), method("m", inputs, endpoint))
			_, endpoints, errs := buildHTTP(e)
			if len(errs) != 0 {
				t.Fatalf("implicit queries rejected: %v", errs)
			}
			if got := endpoints["m"][0].QueryVars; len(got) != 1 || got[0].QueryParamName != "p" {
				t.Fatalf("explicit query template changed: %v", got)
			}
		})
	}
	for _, endpoint := range []Endpoint{GET("/m?tags={tags}&labels={labels}&bytes={bytes}"), POST("/m?tags={tags}&labels={labels}&bytes={bytes}")} {
		e := agent("A", &Mount{Path: "/a/{id}"}, fields("id"), method("m", []engine.Field{
			{Name: "tags", Typ: reflect.TypeFor[[]string]()},
			{Name: "labels", Typ: reflect.TypeFor[map[string]bool]()},
			{Name: "bytes", Typ: reflect.TypeFor[[]uint8]()},
		}, endpoint))
		if _, _, errs := buildHTTP(e); len(errs) != 0 {
			t.Fatalf("explicit collection query rejected: %v", errs)
		}
	}
	for _, typ := range []reflect.Type{reflect.TypeFor[Text](), reflect.TypeFor[Binary](), reflect.TypeFor[[]Text](), reflect.TypeFor[map[string]Text]()} {
		for _, endpoint := range []Endpoint{GET("/m"), POST("/m?payload={payload}")} {
			e := agent("A", &Mount{Path: "/a/{id}"}, fields("id"), method("m", []engine.Field{{Name: "payload", Typ: typ}}, endpoint))
			if _, _, errs := buildHTTP(e); len(errs) == 0 {
				t.Fatalf("rich query %s accepted on %s", typ, endpoint.method)
			}
		}
	}
}

func TestBuildHTTPImplicitQueryExcludesStreamResources(t *testing.T) {
	for _, stream := range []reflect.Type{reflect.TypeFor[AgentStream[byte]](), reflect.TypeFor[AgentStream[string]]()} {
		for _, ep := range []Endpoint{GET("/events"), HEAD("/events"), DELETE("/events"), GET("/events", DurableStreams(StreamRoute{}))} {
			in := []engine.Field{{Name: "uploads", Typ: stream}, {Name: "label", Typ: reflect.TypeFor[string]()}}
			e := agent("Media", &Mount{Path: "/media"}, nil, method("events", in, ep))
			if _, _, errs := buildHTTP(e); len(errs) != 0 {
				t.Fatalf("%s with %s stream rejected: %v", ep.method, stream, errs)
			}
			e = agent("Media", &Mount{Path: "/media"}, nil,
				method("events", in, GET("/events?uploads={uploads}")))
			if _, _, errs := buildHTTP(e); len(errs) == 0 {
				t.Fatalf("explicit stream query binding accepted for %s", stream)
			}
		}
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
	if m.Tag() != common.HttpMethodMethod || m.Method().Tag() != coreHTTP.HttpMethodCustom || m.Method().Custom() != "REPORT" {
		t.Errorf("verb = %v", m)
	}
}

func TestVerbConstructorsMapToWit(t *testing.T) {
	cases := []struct {
		ep  Endpoint
		tag uint8
	}{
		{GET("/x"), coreHTTP.HttpMethodGet},
		{HEAD("/x"), coreHTTP.HttpMethodHead},
		{POST("/x"), coreHTTP.HttpMethodPost},
		{PUT("/x"), coreHTTP.HttpMethodPut},
		{DELETE("/x"), coreHTTP.HttpMethodDelete},
		{CONNECT("/x"), coreHTTP.HttpMethodConnect},
		{OPTIONS("/x"), coreHTTP.HttpMethodOptions},
		{TRACE("/x"), coreHTTP.HttpMethodTrace},
		{PATCH("/x"), coreHTTP.HttpMethodPatch},
	}
	for _, c := range cases {
		if got := witMethod(c.ep.method).Method().Tag(); got != c.tag {
			t.Errorf("%s -> tag %d, want %d", c.ep.method, got, c.tag)
		}
	}
}

// TestBuildHTTPSystemVarAndCatchAllSegments — System-variable and catch-all segments compile on both the mount prefix and an
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
			"implicit records require a body method",
			agent("A", &Mount{Path: "/a/{id}"}, fields("id"), method("m", []engine.Field{{Name: "payload", Typ: reflect.TypeFor[struct{ Name string }]()}}, GET("/m"))),
			`implicit query input "payload"`,
		},
		{
			"duplicate binding",
			agent("A", &Mount{Path: "/a/{id}"}, fields("id"), method("m", fields("x"), POST("/m/{x}?x={x}"))),
			`"x" is bound 2 times`,
		},
		{
			"non-scalar field not bindable",
			agent("A", &Mount{Path: "/a/{id}"}, fields("id"),
				method("m", []engine.Field{{Name: "blob", Typ: reflect.TypeFor[[]byte]()}}, GET("/m/{blob}"))),
			"only scalar fields",
		},
		{
			"catch-all in the mount",
			agent("A", &Mount{Path: "/a/{id}/{*rest}"}, fields("id", "rest")),
			"cannot contain the catch-all {*rest}",
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

func TestAgentIDBindingCompilesToTheMount(t *testing.T) {
	path := agent("Decision", &Mount{Path: "/decisions/{instance}", AgentID: AgentIDPath("instance"), CreateRoute: "/decisions"},
		fields("decision"), method("state", nil, GET("/state")))
	mount, _, errs := buildHTTP(path)
	if len(errs) != 0 {
		t.Fatalf("unexpected definition errors: %v", errs)
	}
	b := mount.Some().AgentIdBinding
	if !b.IsSome() || b.Some().Tag() != common.AgentIdBindingPath || b.Some().Path() != (common.AgentIdBindingDetails{Name: "instance"}) {
		t.Errorf("path selector = %+v", b)
	}
	if !mount.Some().CreateRoute.IsSome() || renderSegs(mount.Some().CreateRoute.Some()) != "/decisions" {
		t.Errorf("create route = %+v", mount.Some().CreateRoute)
	}

	query := agent("Decision", &Mount{Path: "/decisions", AgentID: AgentIDQuery("instance"), CreateRoute: "/{agent-type}/{agent-version}/create"},
		fields("decision"), method("state", nil, GET("/state")))
	mount, _, errs = buildHTTP(query)
	if len(errs) != 0 {
		t.Fatalf("unexpected definition errors: %v", errs)
	}
	b = mount.Some().AgentIdBinding
	if !b.IsSome() || b.Some().Tag() != common.AgentIdBindingQuery || b.Some().Query() != (common.AgentIdBindingDetails{Name: "instance"}) {
		t.Errorf("query selector = %+v", b)
	}
	if !mount.Some().CreateRoute.IsSome() || renderSegs(mount.Some().CreateRoute.Some()) != "/{agent-type}/{agent-version}/create" {
		t.Errorf("create route = %+v", mount.Some().CreateRoute)
	}

	none, _, _ := buildHTTP(agent("Decision", &Mount{Path: "/decisions/{decision}"}, fields("decision")))
	if none.Some().AgentIdBinding.IsSome() || none.Some().CreateRoute.IsSome() {
		t.Errorf("a key mount declares an ID binding or create route")
	}
}

func TestAgentIDBindingValidations(t *testing.T) {
	ephemeral := agent("A", &Mount{Path: "/a", AgentID: AgentIDQuery("instance")}, fields("id"))
	ephemeral.mode = common.AgentModeEphemeral
	streams := method("events", []engine.Field{{Name: "uploads", Typ: reflect.TypeFor[AgentStream[byte]]()}},
		POST("/events", DurableStreams(StreamRoute{})))
	cases := []struct {
		name string
		e    *agentEntry
		want string
	}{
		{"empty name", agent("A", &Mount{Path: "/a", AgentID: AgentIDQuery("")}, fields("id")), "nonempty name"},
		{"ephemeral agent", ephemeral, "requires a durable agent"},
		{
			"fresh agent",
			agent("A", &Mount{Path: "/a", AgentID: AgentIDQuery("instance"), PhantomAgent: true}, fields("id")),
			"cannot be combined with PhantomAgent",
		},
		{"path binding names no capture", agent("A", &Mount{Path: "/a", AgentID: AgentIDPath("instance")}, fields("id")), "exactly one mount capture"},
		{"duplicate ID capture", agent("A", &Mount{Path: "/a/{instance}/{instance}", AgentID: AgentIDPath("instance")}, fields("id")), "exactly one mount capture"},
		{"mixed constructor binding", agent("A", &Mount{Path: "/a/{id}/{instance}", AgentID: AgentIDPath("instance")}, fields("id")), "cannot be combined with constructor"},
		{"query binding with a mount capture", agent("A", &Mount{Path: "/a/{instance}", AgentID: AgentIDQuery("instance")}, fields("id")), "cannot be combined with constructor"},
		{"key-bound create", agent("A", &Mount{Path: "/a/{id}", CreateRoute: "/a"}, fields("id")), "CreateRoute requires AgentID"},
		{"create capture", agent("A", &Mount{Path: "/a/{id}", AgentID: AgentIDPath("id"), CreateRoute: "/a/{id}"}, fields("seed")), "only literal and system"},
		{"create catch-all", agent("A", &Mount{Path: "/a", AgentID: AgentIDQuery("id"), CreateRoute: "/a/{*rest}"}, fields("seed")), "only literal and system"},
		{"create query", agent("A", &Mount{Path: "/a", AgentID: AgentIDQuery("id"), CreateRoute: "/a?x={x}"}, fields("seed")), "query parameters are not allowed"},
		{"create relative", agent("A", &Mount{Path: "/a", AgentID: AgentIDQuery("id"), CreateRoute: "create"}, fields("seed")), "path must start"},
		{
			"endpoint path variable",
			agent("A", &Mount{Path: "/a/{instance}", AgentID: AgentIDPath("instance")}, fields("id"), method("m", fields("instance"), POST("/m/{instance}"))),
			"is the mount's AgentID binding",
		},
		{
			"endpoint query parameter",
			agent("A", &Mount{Path: "/a", AgentID: AgentIDQuery("instance")}, fields("id"), method("m", fields("x"), POST("/m?instance={x}"))),
			"is the mount's AgentID binding",
		},
		{"implicit query", agent("A", &Mount{Path: "/a", AgentID: AgentIDQuery("instance")}, fields("id"), method("m", fields("instance"), GET("/m"))), "implicit query input"},
		{"implicit HEAD query", agent("A", &Mount{Path: "/a", AgentID: AgentIDQuery("instance")}, fields("id"), method("m", fields("instance"), HEAD("/m"))), "implicit query input"},
		{"implicit DELETE query", agent("A", &Mount{Path: "/a", AgentID: AgentIDQuery("instance")}, fields("id"), method("m", fields("instance"), DELETE("/m"))), "implicit query input"},
		{"durable streams control", agent("A", &Mount{Path: "/a", AgentID: AgentIDQuery("offset")}, fields("id"), streams), "Durable Streams control"},
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

func TestAgentIDBindingAllowsFilesWithoutConstructorCaptures(t *testing.T) {
	for _, mount := range []*Mount{
		{Path: "/a/{instance}", AgentID: AgentIDPath("instance"), CreateRoute: "/a"},
		{Path: "/a", AgentID: AgentIDQuery("instance"), CreateRoute: "/a/create"},
	} {
		mount.ExposeFiles = []FileMapping{{Route: "/files", Path: "/data"}}
		// Even rich constructor inputs are irrelevant to ID-based addressing.
		e := agent("A", mount, []engine.Field{{Name: "seed", Typ: reflect.TypeFor[map[string]string]()}},
			method("state", fields("limit"), GET("/state")))
		got, endpoints, errs := buildHTTP(e)
		if len(errs) != 0 {
			t.Fatalf("%s: unexpected errors: %v", mount.Path, errs)
		}
		if len(got.Some().FilesystemBindings) != 1 || len(endpoints["state"]) != 1 {
			t.Fatalf("file or method metadata missing: %+v, %+v", got, endpoints)
		}
	}
}

func TestAgentIDQueryControlsFollowEffectiveTransport(t *testing.T) {
	for _, name := range []string{"offset", "cursor", "live"} {
		for _, output := range []bool{false, true} {
			for _, mode := range []RouteMode{"", RouteDurableStreams, RouteSSE} {
				if !output && mode == RouteSSE {
					continue // SSE requires an output stream.
				}
				ep := GET("/events")
				if mode != "" {
					ep = GET("/events", WithRouteMode(mode))
				}
				m := method("events", nil, ep)
				if output {
					m.outType = reflect.TypeFor[AgentStream[string]]()
				} else {
					m.inFields = []engine.Field{{Name: "uploads", Typ: reflect.TypeFor[AgentStream[byte]]()}}
				}
				_, _, errs := buildHTTP(agent("A", &Mount{Path: "/a", AgentID: AgentIDQuery(name)}, fields("seed"), m))
				conflict := mode != RouteSSE
				if containsDefErr(errs, "Durable Streams control") != conflict || !conflict && len(errs) != 0 {
					t.Errorf("%s output=%v mode=%s: errors %v, want conflict=%v", name, output, mode, errs, conflict)
				}
			}
		}
		_, _, errs := buildHTTP(agent("A", &Mount{Path: "/a", AgentID: AgentIDQuery(name)}, fields("seed"), method("plain", nil, GET("/plain"))))
		if len(errs) != 0 {
			t.Errorf("plain route with %s: %v", name, errs)
		}
	}
}

func TestAgentIDBindingExcludesInputStreams(t *testing.T) {
	for _, verb := range []func(string, ...EndpointOpt) Endpoint{GET, HEAD, DELETE} {
		for _, typ := range []reflect.Type{reflect.TypeFor[AgentStream[byte]](), reflect.TypeFor[AgentStream[string]]()} {
			for _, explicit := range []bool{false, true} {
				ep := verb("/events")
				if explicit {
					ep = verb("/events", Query("instance", "instance"))
				}
				e := agent("A", &Mount{Path: "/a", AgentID: AgentIDQuery("instance")}, fields("seed"),
					method("events", []engine.Field{{Name: "instance", Typ: typ}}, ep))
				_, _, errs := buildHTTP(e)
				if (len(errs) != 0) != explicit {
					t.Errorf("%s stream %s explicit=%v: errors %v", ep.method, typ, explicit, errs)
				}
			}
		}
	}
}

func TestAgentIDBindingStreamDeclarationPublishes(t *testing.T) {
	type Id struct{ Seed string }
	type State struct{}
	type Input struct{ Instance AgentStream[string] }
	withDefs(t, func(d *definitions) {
		a := defineAgentInto[Id, NoConfig](d, Spec{Name: "StreamSelected", HTTP: &Mount{Path: "/a", AgentID: AgentIDQuery("instance")}})
		m := a.Method[Input, Unit]("events", HTTP(GET("/events")))
		impl := implementInto[Id, State, NoConfig](d, a, simpleNewState[Id, State](func(Id) *State { return &State{} }), false)
		impl.Handle(m, func(*Context[State], Input) Unit { return Unit{} })
		types, errs := d.discover()
		if len(errs) != 0 || len(types) != 1 || len(types[0].Methods[0].HttpEndpoint) != 1 {
			t.Fatalf("stream declaration failed to publish: %v, %v", types, errs)
		}
	})
}

func TestAgentIDBindingDoesNotReserveOtherInputSources(t *testing.T) {
	for _, ep := range []Endpoint{
		POST("/state"), GET("/state/{instance}"),
		GET("/state", Header("X-Instance", "instance")),
		GET("/state", Query("other", "instance")),
	} {
		e := agent("A", &Mount{Path: "/a", AgentID: AgentIDQuery("instance")}, fields("seed"),
			method("state", fields("instance"), ep))
		_, _, errs := buildHTTP(e)
		if len(errs) != 0 {
			t.Errorf("%s %s: unexpected errors: %v", ep.method, ep.path, errs)
		}
	}
	// A capture with the same name as a constructor field is still an ID
	// binding; it does not bind that constructor field.
	e := agent("A", &Mount{Path: "/a/{id}", AgentID: AgentIDPath("id"), CreateRoute: "/"}, fields("id"))
	mount, _, errs := buildHTTP(e)
	if len(errs) != 0 || !mount.Some().CreateRoute.IsSome() || len(mount.Some().CreateRoute.Some()) != 0 {
		t.Fatalf("root create route and ID binding: %+v, %v", mount, errs)
	}
}

func TestNonIDMountLifecycleIsUnchanged(t *testing.T) {
	for _, mode := range []common.AgentMode{common.AgentModeDurable, common.AgentModeEphemeral} {
		for _, fresh := range []bool{false, true} {
			e := agent("A", &Mount{Path: "/a/{seed}", PhantomAgent: fresh}, fields("seed"))
			e.mode = mode
			mount, _, errs := buildHTTP(e)
			if len(errs) != 0 || mount.Some().PhantomAgent != fresh || mount.Some().AgentIdBinding.IsSome() {
				t.Errorf("mode=%v fresh=%v: %+v, %v", mode, fresh, mount, errs)
			}
		}
	}
}

func TestRouterRejectsIDBindingAndCreate(t *testing.T) {
	for _, mount := range []*Mount{
		{Path: "/a", AgentID: AgentIDQuery("id")},
		{Path: "/a", CreateRoute: "/create"},
	} {
		e := agent("A", mount, nil)
		e.router = &routerEntry{}
		_, _, errs := buildHTTP(e)
		if !containsDefErr(errs, "router mount cannot declare AgentID or CreateRoute") {
			t.Errorf("expected router ownership error, got %v", errs)
		}
	}
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

func fields(names ...string) []engine.Field {
	out := make([]engine.Field, len(names))
	for i, n := range names {
		out[i] = engine.Field{Name: n, Typ: reflect.TypeFor[string]()}
	}
	return out
}

func method(name string, in []engine.Field, eps ...Endpoint) *methodEntry {
	return &methodEntry{name: name, inFields: in, endpoints: eps}
}

func agent(name string, mount *Mount, idFields []engine.Field, methods ...*methodEntry) *agentEntry {
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
		case common.PathSegmentSegment:
			shared := s.Segment()
			switch shared.Tag() {
			case coreHTTP.PathSegmentLiteral:
				parts = append(parts, shared.Literal())
			case coreHTTP.PathSegmentPathVariable:
				parts = append(parts, "{"+shared.PathVariable().VariableName+"}")
			case coreHTTP.PathSegmentRemainingPathVariable:
				parts = append(parts, "{*"+shared.RemainingPathVariable().VariableName+"}")
			}
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

func TestToolMountRejectsAgentSelectionAndCreation(t *testing.T) {
	plain, err := (Mount{Path: "/reports"}).ToolDeclaration()
	if err != nil || !reflect.DeepEqual(plain.PathPrefix, []coreHTTP.PathSegment{coreHTTP.MakePathSegmentLiteral("reports")}) {
		t.Fatalf("plain tool mount: %v, %v", plain, err)
	}
	for _, mount := range []Mount{
		{Path: "/reports", AgentID: AgentIDQuery("instance")},
		{Path: "/reports/{instance}", AgentID: AgentIDPath("instance"), CreateRoute: "/reports"},
		{Path: "/reports", CreateRoute: "/"},
	} {
		if _, err := mount.ToolDeclaration(); err == nil || !strings.Contains(err.Error(), "only a path") {
			t.Errorf("agent-only tool mount accepted: %+v, %v", mount, err)
		}
	}
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

func containsDefErr(errs []engine.DefError, want string) bool {
	for _, e := range errs {
		if strings.Contains(e.Error(), want) {
			return true
		}
	}
	return false
}
