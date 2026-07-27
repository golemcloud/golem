package golem

import (
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
