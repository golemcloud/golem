package roundtrip

import (
	"errors"
	"fmt"
	"io"
	"net/http"
	"strconv"
	"strings"

	go_wasi_http "golem.com/tinygo_wasi/tinygo_wasi"
)

type WasiHttpTransport struct {
}

func (t WasiHttpTransport) RoundTrip(request *http.Request) (*http.Response, error) {

	var headerKeyValues []go_wasi_http.WasiHttp0_2_0_TypesTuple2FieldKeyFieldValueT
	for key, values := range request.Header {
		for _, value := range values {
			headerKeyValues = append(headerKeyValues, go_wasi_http.WasiHttp0_2_0_TypesTuple2FieldKeyFieldValueT{
				F0: key,
				F1: []byte(value),
			})
		}
	}
	headers := go_wasi_http.StaticFieldsFromList(headerKeyValues).Unwrap()

	var method go_wasi_http.WasiHttp0_2_0_TypesMethod
	switch strings.ToUpper(request.Method) {
	case "":
		method = go_wasi_http.WasiHttp0_2_0_TypesMethodGet()
	case "GET":
		method = go_wasi_http.WasiHttp0_2_0_TypesMethodGet()
	case "HEAD":
		method = go_wasi_http.WasiHttp0_2_0_TypesMethodHead()
	case "POST":
		method = go_wasi_http.WasiHttp0_2_0_TypesMethodPost()
	case "PUT":
		method = go_wasi_http.WasiHttp0_2_0_TypesMethodPut()
	case "DELETE":
		method = go_wasi_http.WasiHttp0_2_0_TypesMethodDelete()
	case "CONNECT":
		method = go_wasi_http.WasiHttp0_2_0_TypesMethodConnect()
	case "OPTIONS":
		method = go_wasi_http.WasiHttp0_2_0_TypesMethodOptions()
	case "TRACE":
		method = go_wasi_http.WasiHttp0_2_0_TypesMethodTrace()
	case "PATCH":
		method = go_wasi_http.WasiHttp0_2_0_TypesMethodPatch()
	default:
		method = go_wasi_http.WasiHttp0_2_0_TypesMethodOther(request.Method)
	}

	path := request.URL.Path
	query := request.URL.RawQuery
	pathAndQuery := path
	if query != "" {
		pathAndQuery += "?" + query
	}

	var scheme go_wasi_http.WasiHttp0_2_0_TypesScheme
	switch strings.ToLower(request.URL.Scheme) {
	case "http":
		scheme = go_wasi_http.WasiHttp0_2_0_TypesSchemeHttp()
	case "https":
		scheme = go_wasi_http.WasiHttp0_2_0_TypesSchemeHttps()
	default:
		scheme = go_wasi_http.WasiHttp0_2_0_TypesSchemeOther(request.URL.Scheme)
	}

	userPassword := request.URL.User.String()
	var authority string
	if userPassword == "" {
		authority = request.URL.Host
	} else {
		authority = userPassword + "@" + request.URL.Host
	}

	requestHandle := go_wasi_http.NewOutgoingRequest(headers)

	requestHandle.SetMethod(method)
	requestHandle.SetPathWithQuery(go_wasi_http.Some(pathAndQuery))
	requestHandle.SetScheme(go_wasi_http.Some(scheme))
	requestHandle.SetAuthority(go_wasi_http.Some(authority))

	if request.Body != nil {
		reader := request.Body
		defer reader.Close()

		requestBodyResult := requestHandle.Body()
		if requestBodyResult.IsErr() {
			return nil, errors.New("Failed to get request body")
		}
		requestBody := requestBodyResult.Unwrap()

		requestStreamResult := requestBody.Write()
		if requestStreamResult.IsErr() {
			return nil, errors.New("Failed to start writing request body")
		}
		requestStream := requestStreamResult.Unwrap()

		buffer := make([]byte, 1024)
		for {
			n, err := reader.Read(buffer)

			result := requestStream.Write(buffer[:n])
			if result.IsErr() {
				requestStream.Drop()
				requestBody.Drop()
				return nil, errors.New("Failed to write request body chunk")
			}

			if err == io.EOF {
				break
			}
		}

		requestStream.Drop()
		go_wasi_http.StaticOutgoingBodyFinish(requestBody, go_wasi_http.None[go_wasi_http.WasiHttp0_2_0_TypesTrailers]())
		// requestBody.Drop() // TODO: this fails with "unknown handle index 0"
	}

	// TODO: timeouts
	connectTimeoutNanos := go_wasi_http.None[uint64]()
	firstByteTimeoutNanos := go_wasi_http.None[uint64]()
	betweenBytesTimeoutNanos := go_wasi_http.None[uint64]()
	options := go_wasi_http.NewRequestOptions()
	options.SetConnectTimeout(connectTimeoutNanos)
	options.SetFirstByteTimeout(firstByteTimeoutNanos)
	options.SetBetweenBytesTimeout(betweenBytesTimeoutNanos)

	futureResult := go_wasi_http.WasiHttp0_2_0_OutgoingHandlerHandle(requestHandle, go_wasi_http.Some(options))
	if futureResult.IsErr() {
		return nil, errors.New("Failed to send request")
	}
	future := futureResult.Unwrap()

	incomingResponse, err := GetIncomingResponse(future)
	if err != nil {
		return nil, err
	}

	status := incomingResponse.Status()
	responseHeaders := incomingResponse.Headers()
	defer responseHeaders.Drop()

	responseHeaderEntries := responseHeaders.Entries()
	header := http.Header{}

	for _, tuple := range responseHeaderEntries {
		ck := http.CanonicalHeaderKey(tuple.F0)
		header[ck] = append(header[ck], string(tuple.F1))
	}

	var contentLength int64
	clHeader := header.Get("Content-Length")
	switch {
	case clHeader != "":
		cl, err := strconv.ParseInt(clHeader, 10, 64)
		if err != nil {
			return nil, fmt.Errorf("net/http: ill-formed Content-Length header: %v", err)
		}
		if cl < 0 {
			// Content-Length values less than 0 are invalid.
			// See: https://datatracker.ietf.org/doc/html/rfc2616/#section-14.13
			return nil, fmt.Errorf("net/http: invalid Content-Length header: %q", clHeader)
		}
		contentLength = cl
	default:
		// If the response length is not declared, set it to -1.
		contentLength = -1
	}

	responseBodyResult := incomingResponse.Consume()
	if responseBodyResult.IsErr() {
		return nil, errors.New("Failed to consume response body")
	}
	responseBody := responseBodyResult.Unwrap()

	responseBodyStreamResult := responseBody.Stream()
	if responseBodyStreamResult.IsErr() {
		return nil, errors.New("Failed to get response body stream")
	}
	responseBodyStream := responseBodyStreamResult.Unwrap()

	responseReader := WasiStreamReader{
		Stream:           responseBodyStream,
		Body:             responseBody,
		OutgoingRequest:  requestHandle,
		IncomingResponse: incomingResponse,
		Future:           future,
	}

	response := http.Response{
		Status:        fmt.Sprintf("%d %s", status, http.StatusText(int(status))),
		StatusCode:    int(status),
		Header:        header,
		ContentLength: contentLength,
		Body:          responseReader,
		Request:       request,
	}

	return &response, nil
}

func GetIncomingResponse(future go_wasi_http.WasiHttp0_2_0_OutgoingHandlerFutureIncomingResponse) (go_wasi_http.WasiHttp0_2_0_TypesIncomingResponse, error) {
	result := future.Get()
	if result.IsSome() {
		result2 := result.Unwrap()
		if result2.IsErr() {
			return 0, errors.New("Failed to send request")
		}
		result3 := result2.Unwrap()
		if result3.IsErr() {
			return 0, errors.New("Failed to send request")
		}
		return result3.Unwrap(), nil
	} else {
		pollable := future.Subscribe()
		pollable.Block()
		return GetIncomingResponse(future)
	}
}

type WasiStreamReader struct {
	Stream           go_wasi_http.WasiHttp0_2_0_TypesInputStream
	Body             go_wasi_http.WasiHttp0_2_0_TypesIncomingBody
	OutgoingRequest  go_wasi_http.WasiHttp0_2_0_TypesOutgoingRequest
	IncomingResponse go_wasi_http.WasiHttp0_2_0_TypesIncomingResponse
	Future           go_wasi_http.WasiHttp0_2_0_TypesFutureIncomingResponse
}

func (reader WasiStreamReader) Read(p []byte) (int, error) {
	c := cap(p)
	result := reader.Stream.BlockingRead(uint64(c))
	isEof := result.IsErr() && result.UnwrapErr() == go_wasi_http.WasiIo0_2_0_StreamsStreamErrorClosed()
	if isEof {
		return 0, io.EOF
	} else if result.IsErr() {
		return 0, errors.New("Failed to read response stream")
	} else {
		chunk := result.Unwrap()
		copy(p, chunk)
		return len(chunk), nil
	}
}

func (reader WasiStreamReader) Close() error {
	reader.Stream.Drop()
	reader.Body.Drop()
	reader.IncomingResponse.Drop()
	reader.Future.Drop()
	reader.OutgoingRequest.Drop()
	return nil
}
