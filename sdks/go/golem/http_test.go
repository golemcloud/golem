package golem

import (
	"net/http"
	"testing"
	"time"

	httptypes "github.com/golemcloud/golem/sdks/go/golem/internal/wit/wasi_http_0_3_0_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

func TestTimeoutFromDeadline(t *testing.T) {
	now := time.Unix(1000, 0)

	if nanos, ok := timeoutFromDeadline(now.Add(2*time.Second), now); !ok || nanos != uint64(2*time.Second) {
		t.Fatalf("future deadline: got (%d, %v), want (%d, true)", nanos, ok, uint64(2*time.Second))
	}
	if _, ok := timeoutFromDeadline(now, now); ok {
		t.Fatal("deadline == now should yield no budget")
	}
	if _, ok := timeoutFromDeadline(now.Add(-time.Second), now); ok {
		t.Fatal("past deadline should yield no budget")
	}
}

func TestErrorCodeStringCoversFamilies(t *testing.T) {
	cases := map[httptypes.ErrorCode]string{
		httptypes.MakeErrorCodeConnectionRefused():                    "connection refused",
		httptypes.MakeErrorCodeConnectionTimeout():                    "connection timeout",
		httptypes.MakeErrorCodeTlsCertificateError():                  "TLS certificate error",
		httptypes.MakeErrorCodeHttpProtocolError():                    "HTTP protocol error",
		httptypes.MakeErrorCodeDnsTimeout():                           "DNS timeout",
		httptypes.MakeErrorCodeInternalError(witTypes.None[string]()): "internal error",
	}
	for code, want := range cases {
		if got := errorCodeString(code); got != want {
			t.Errorf("errorCodeString(tag %d) = %q, want %q", code.Tag(), got, want)
		}
	}
}

// TestMethodOf — methodOf maps an HTTP method string onto the wasi method variant, defaulting a
// blank method to GET and passing anything unknown through as other(name).
func TestMethodOf(t *testing.T) {
	known := map[string]httptypes.Method{
		"":        httptypes.MakeMethodGet(),
		"get":     httptypes.MakeMethodGet(),
		"GET":     httptypes.MakeMethodGet(),
		"head":    httptypes.MakeMethodHead(),
		"POST":    httptypes.MakeMethodPost(),
		"put":     httptypes.MakeMethodPut(),
		"DELETE":  httptypes.MakeMethodDelete(),
		"connect": httptypes.MakeMethodConnect(),
		"OPTIONS": httptypes.MakeMethodOptions(),
		"trace":   httptypes.MakeMethodTrace(),
		"PATCH":   httptypes.MakeMethodPatch(),
	}
	for in, want := range known {
		if got := methodOf(in); got.Tag() != want.Tag() {
			t.Errorf("methodOf(%q) tag = %d, want %d", in, got.Tag(), want.Tag())
		}
	}

	// An unknown method is preserved verbatim as other(name).
	other := methodOf("PROPFIND")
	if other.Tag() != httptypes.MakeMethodOther("PROPFIND").Tag() || other.Other() != "PROPFIND" {
		t.Errorf("methodOf(PROPFIND) = tag %d / %q, want other(PROPFIND)", other.Tag(), other.Other())
	}
}

// TestSchemeOf — schemeOf maps a URL scheme onto the wasi scheme variant; a blank scheme
// defaults to https and anything unknown passes through as other(name).
func TestSchemeOf(t *testing.T) {
	if got := schemeOf("http"); got.Tag() != httptypes.MakeSchemeHttp().Tag() {
		t.Errorf("schemeOf(http) tag = %d, want http", got.Tag())
	}
	for _, in := range []string{"https", "HTTPS", ""} {
		if got := schemeOf(in); got.Tag() != httptypes.MakeSchemeHttps().Tag() {
			t.Errorf("schemeOf(%q) tag = %d, want https", in, got.Tag())
		}
	}
	other := schemeOf("ftp")
	if other.Tag() != httptypes.MakeSchemeOther("ftp").Tag() || other.Other() != "ftp" {
		t.Errorf("schemeOf(ftp) = tag %d / %q, want other(ftp)", other.Tag(), other.Other())
	}
}

// TestContentLength — contentLength reads the Content-Length header, returning -1 (unknown) when it
// is absent or unparseable.
func TestContentLength(t *testing.T) {
	if got := contentLength(http.Header{}); got != -1 {
		t.Errorf("missing Content-Length = %d, want -1", got)
	}
	if got := contentLength(http.Header{"Content-Length": {"42"}}); got != 42 {
		t.Errorf("Content-Length 42 = %d, want 42", got)
	}
	if got := contentLength(http.Header{"Content-Length": {"nope"}}); got != -1 {
		t.Errorf("unparseable Content-Length = %d, want -1", got)
	}
}

// TestHeaderErr — headerErr renders each wasi header-error case to a readable string.
func TestHeaderErr(t *testing.T) {
	cases := map[httptypes.HeaderError]string{
		httptypes.MakeHeaderErrorInvalidSyntax(): "invalid syntax",
		httptypes.MakeHeaderErrorForbidden():     "forbidden",
		httptypes.MakeHeaderErrorImmutable():     "immutable",
	}
	for e, want := range cases {
		if got := headerErr(e); got != want {
			t.Errorf("headerErr(tag %d) = %q, want %q", e.Tag(), got, want)
		}
	}
}
