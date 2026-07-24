// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

package golem

import (
	"fmt"
	"io"
	"net/http"
	"strings"

	httpclient "github.com/golemcloud/golem/sdks/go/golem/internal/wit/wasi_http_0_3_0_client"
	httptypes "github.com/golemcloud/golem/sdks/go/golem/internal/wit/wasi_http_0_3_0_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// Outgoing HTTP.
//
// On this target Go's default transport is inert: wasip1 has no sockets, so
// net.Dial never connects. This RoundTripper routes net/http through
// wasi:http/client@0.3.0 instead, and the SDK installs it as
// http.DefaultTransport in init() — so http.Get, http.DefaultClient, and any
// library built on them work with no wiring from the user.
//
// Durability (idempotency keys, retries, trace propagation, accounting) is the
// host's responsibility; this transport only marshals request/response.
//
// Not supported: anything that bypasses the transport and opens a raw socket —
// net.Dial, most SQL drivers, custom gRPC transports. Those cannot work in a
// component; use the SDK's typed host wrappers instead.

// Transport is an http.RoundTripper backed by wasi:http/client@0.3.0.
// The zero value is ready to use; the SDK installs one as http.DefaultTransport.
type Transport struct{}

// RoundTrip implements http.RoundTripper.
func (Transport) RoundTrip(req *http.Request) (*http.Response, error) {
	headers := httptypes.MakeFields()
	for name, values := range req.Header {
		for _, v := range values {
			if r := headers.Append(name, []byte(v)); r.IsErr() {
				return nil, fmt.Errorf("golem/http: invalid header %q: %v", name, headerErr(r.Err()))
			}
		}
	}
	// Host expects a Host header via the authority; don't duplicate it here.
	headers.GetAndDelete("Host")

	contents := requestBody(req)

	// No request trailers. The future write blocks until the host consumes it,
	// and only the blocking Send below drives the event loop — so resolve it
	// from a goroutine, exactly like the request body stream.
	trailersW, trailersR := httptypes.MakeFutureResultOptionFieldsErrorCode()
	go trailersW.Write(witTypes.Ok[witTypes.Option[*httptypes.Fields], httptypes.ErrorCode](
		witTypes.None[*httptypes.Fields]()))

	request, _ := httptypes.RequestNew(
		headers,
		contents,
		trailersR,
		witTypes.None[*httptypes.RequestOptions](),
	)

	request.SetMethod(methodOf(req.Method))
	if req.URL != nil {
		request.SetScheme(witTypes.Some(schemeOf(req.URL.Scheme)))
		if req.URL.Host != "" {
			request.SetAuthority(witTypes.Some(req.URL.Host))
		}
		request.SetPathWithQuery(witTypes.Some(req.URL.RequestURI()))
	}

	// Blocking from Go's view; a goroutine writing the request body yields to
	// the component-model event loop while the host drains the stream.
	res := httpclient.Send(request)
	if res.IsErr() {
		return nil, fmt.Errorf("golem/http: %s %s: %s", req.Method, req.URL, errorCodeString(res.Err()))
	}
	resp := res.Ok()

	header := http.Header{}
	for _, kv := range resp.GetHeaders().CopyAll() {
		header.Add(kv.F0, string(kv.F1))
	}

	status := int(resp.GetStatusCode())

	// Consume the response body as a stream wrapped in an io.ReadCloser. The
	// "handled ok" future is resolved from a goroutine for the same reason as
	// the trailers future — its write blocks until the host consumes it, which
	// happens while we read the body stream below.
	doneW, doneR := httptypes.MakeFutureResultUnitErrorCode()
	go doneW.Write(witTypes.Ok[witTypes.Unit, httptypes.ErrorCode](witTypes.Unit{}))
	bodyStream, _ := httptypes.ResponseConsumeBody(resp, doneR)

	return &http.Response{
		StatusCode:    status,
		Status:        fmt.Sprintf("%d %s", status, http.StatusText(status)),
		Proto:         "HTTP/1.1",
		ProtoMajor:    1,
		ProtoMinor:    1,
		Header:        header,
		Body:          &responseBody{stream: bodyStream},
		ContentLength: contentLength(header),
		Request:       req,
	}, nil
}

// requestBody turns req.Body into an optional stream the host reads from. A
// goroutine copies the body into the stream writer; it runs concurrently with
// the blocking Send because a goroutine blocked on WriteAll yields via
// wasiOnIdle.
func requestBody(req *http.Request) witTypes.Option[*witTypes.StreamReader[uint8]] {
	if req.Body == nil || req.Body == http.NoBody {
		return witTypes.None[*witTypes.StreamReader[uint8]]()
	}
	w, r := httptypes.MakeStreamU8()
	go func() {
		defer req.Body.Close()
		defer w.Drop()
		buf := make([]byte, 16*1024)
		for {
			n, err := req.Body.Read(buf)
			if n > 0 {
				w.WriteAll(buf[:n])
			}
			if err != nil {
				return
			}
		}
	}()
	return witTypes.Some(r)
}

// responseBody adapts a wasi stream<u8> to io.ReadCloser.
type responseBody struct {
	stream *witTypes.StreamReader[uint8]
	buf    []byte // leftover bytes from the last stream read
}

func (b *responseBody) Read(p []byte) (int, error) {
	if len(b.buf) == 0 {
		chunk := make([]byte, len(p))
		n := b.stream.Read(chunk)
		if n == 0 {
			if b.stream.WriterDropped() {
				return 0, io.EOF
			}
			return 0, nil
		}
		b.buf = chunk[:n]
	}
	n := copy(p, b.buf)
	b.buf = b.buf[n:]
	return n, nil
}

func (b *responseBody) Close() error {
	b.stream.Drop()
	return nil
}

func methodOf(m string) httptypes.Method {
	switch strings.ToUpper(m) {
	case "", "GET":
		return httptypes.MakeMethodGet()
	case "HEAD":
		return httptypes.MakeMethodHead()
	case "POST":
		return httptypes.MakeMethodPost()
	case "PUT":
		return httptypes.MakeMethodPut()
	case "DELETE":
		return httptypes.MakeMethodDelete()
	case "CONNECT":
		return httptypes.MakeMethodConnect()
	case "OPTIONS":
		return httptypes.MakeMethodOptions()
	case "TRACE":
		return httptypes.MakeMethodTrace()
	case "PATCH":
		return httptypes.MakeMethodPatch()
	default:
		return httptypes.MakeMethodOther(m)
	}
}

func schemeOf(s string) httptypes.Scheme {
	switch strings.ToLower(s) {
	case "http":
		return httptypes.MakeSchemeHttp()
	case "https", "":
		return httptypes.MakeSchemeHttps()
	default:
		return httptypes.MakeSchemeOther(s)
	}
}

func contentLength(h http.Header) int64 {
	if cl := h.Get("Content-Length"); cl != "" {
		var n int64
		if _, err := fmt.Sscan(cl, &n); err == nil {
			return n
		}
	}
	return -1
}

func headerErr(e httptypes.HeaderError) string {
	switch e.Tag() {
	case httptypes.HeaderErrorInvalidSyntax:
		return "invalid syntax"
	case httptypes.HeaderErrorForbidden:
		return "forbidden"
	case httptypes.HeaderErrorImmutable:
		return "immutable"
	default:
		return fmt.Sprintf("header error (tag %d)", e.Tag())
	}
}

// errorCodeString renders the wasi:http error-code enough for a Go error.
func errorCodeString(e httptypes.ErrorCode) string {
	switch e.Tag() {
	case httptypes.ErrorCodeDestinationNotFound:
		return "destination not found"
	case httptypes.ErrorCodeDestinationUnavailable:
		return "destination unavailable"
	case httptypes.ErrorCodeConnectionRefused:
		return "connection refused"
	case httptypes.ErrorCodeConnectionTerminated:
		return "connection terminated"
	case httptypes.ErrorCodeConnectionTimeout:
		return "connection timeout"
	case httptypes.ErrorCodeInternalError:
		return "internal error"
	default:
		return fmt.Sprintf("http error (tag %d)", e.Tag())
	}
}

func init() {
	// Route net/http through wasi:http/client. The default transport can't dial
	// on this target, so any code using http.DefaultClient depends on this.
	http.DefaultClient.Transport = Transport{}
	http.DefaultTransport = Transport{}
}
