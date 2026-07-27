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
	"io"
	"net/http"
	"strings"
	"time"

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
//
// Timeouts: set one with a request context deadline —
// http.NewRequestWithContext(context.WithTimeout(...)). That deadline is
// translated into wasi:http request-options and enforced by the host. Note that
// http.Client.Timeout does NOT work here: the stdlib enforces it from a client
// side timer goroutine, which can't preempt the blocking host Send on this
// single-threaded target, and it never stamps req.Context().Deadline() for the
// transport to read. Use a context deadline instead.
func (Transport) RoundTrip(req *http.Request) (*http.Response, error) {
	// A context deadline (or an already-cancelled context) lands on req; honor a
	// cancellation before doing any work.
	if err := req.Context().Err(); err != nil {
		return nil, err
	}

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

	// Translate the request's context deadline into wasi:http request-options.
	// One budget bounds each transport phase.
	options := witTypes.None[*httptypes.RequestOptions]()
	if dl, ok := req.Context().Deadline(); ok {
		if nanos, set := timeoutFromDeadline(dl, time.Now()); set {
			o := httptypes.MakeRequestOptions()
			o.SetConnectTimeout(witTypes.Some(nanos))
			o.SetFirstByteTimeout(witTypes.Some(nanos))
			o.SetBetweenBytesTimeout(witTypes.Some(nanos))
			options = witTypes.Some(o)
		}
	}

	request, _ := httptypes.RequestNew(
		headers,
		contents,
		trailersR,
		options,
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

// timeoutFromDeadline returns the remaining time until deadline in nanoseconds,
// and whether a positive budget remains. Pure (clock passed in) so it is
// natively testable.
func timeoutFromDeadline(deadline, now time.Time) (uint64, bool) {
	remaining := deadline.Sub(now)
	if remaining <= 0 {
		return 0, false
	}
	return uint64(remaining.Nanoseconds()), true
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

// errorCodeString renders a wasi:http error-code into a readable message. It
// covers the meaningful families (DNS, destination, connection, TLS,
// request/response protocol, size/policy limits) and falls back to the tag for
// anything else.
func errorCodeString(e httptypes.ErrorCode) string {
	switch e.Tag() {
	// DNS
	case httptypes.ErrorCodeDnsTimeout:
		return "DNS timeout"
	case httptypes.ErrorCodeDnsError:
		return "DNS error"
	// destination
	case httptypes.ErrorCodeDestinationNotFound:
		return "destination not found"
	case httptypes.ErrorCodeDestinationUnavailable:
		return "destination unavailable"
	case httptypes.ErrorCodeDestinationIpProhibited:
		return "destination IP prohibited"
	case httptypes.ErrorCodeDestinationIpUnroutable:
		return "destination IP unroutable"
	// connection
	case httptypes.ErrorCodeConnectionRefused:
		return "connection refused"
	case httptypes.ErrorCodeConnectionTerminated:
		return "connection terminated"
	case httptypes.ErrorCodeConnectionTimeout:
		return "connection timeout"
	case httptypes.ErrorCodeConnectionReadTimeout:
		return "connection read timeout"
	case httptypes.ErrorCodeConnectionWriteTimeout:
		return "connection write timeout"
	case httptypes.ErrorCodeConnectionLimitReached:
		return "connection limit reached"
	// TLS
	case httptypes.ErrorCodeTlsProtocolError:
		return "TLS protocol error"
	case httptypes.ErrorCodeTlsCertificateError:
		return "TLS certificate error"
	case httptypes.ErrorCodeTlsAlertReceived:
		return "TLS alert received"
	// request / response protocol
	case httptypes.ErrorCodeHttpRequestDenied:
		return "HTTP request denied"
	case httptypes.ErrorCodeHttpRequestUriInvalid:
		return "HTTP request URI invalid"
	case httptypes.ErrorCodeHttpRequestUriTooLong:
		return "HTTP request URI too long"
	case httptypes.ErrorCodeHttpResponseIncomplete:
		return "HTTP response incomplete"
	case httptypes.ErrorCodeHttpResponseTimeout:
		return "HTTP response timeout"
	case httptypes.ErrorCodeHttpUpgradeFailed:
		return "HTTP upgrade failed"
	case httptypes.ErrorCodeHttpProtocolError:
		return "HTTP protocol error"
	// size / policy limits
	case httptypes.ErrorCodeHttpRequestBodySize:
		return "HTTP request body too large"
	case httptypes.ErrorCodeHttpResponseBodySize:
		return "HTTP response body too large"
	case httptypes.ErrorCodeHttpResponseHeaderSize, httptypes.ErrorCodeHttpResponseHeaderSectionSize:
		return "HTTP response headers too large"
	case httptypes.ErrorCodeLoopDetected:
		return "loop detected"
	case httptypes.ErrorCodeConfigurationError:
		return "configuration error"
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
