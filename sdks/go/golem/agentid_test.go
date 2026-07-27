package golem

import (
	"reflect"
	"testing"

	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
)

// The host `parse-agent-id` call can't link natively, but the decode of its
// constructor-param value tree into the typed Id can — and is where a bug would
// hide. Round-trip: encode an Id's fields, then decode via decodeAgentIDParams.
type parsedID struct {
	Merchant string
	Region   int32
}

func TestDecodeAgentIDParamsRoundTrip(t *testing.T) {
	want := parsedID{Merchant: "acme", Region: 7}
	fields := structFields(reflect.TypeFor[parsedID]())
	tree := encodeParams(fields, reflect.ValueOf(&want).Elem())

	got, err := decodeAgentIDParams[parsedID](tree)
	if err != nil {
		t.Fatalf("decode: %v", err)
	}
	if got != want {
		t.Fatalf("round trip = %#v, want %#v", got, want)
	}
}

func TestDecodeAgentIDParamsRejectsNonStruct(t *testing.T) {
	if _, err := decodeAgentIDParams[string](types.SchemaValueTree{}); err == nil {
		t.Fatal("a non-struct Id should be rejected")
	}
}

func TestUUIDFromWitAndString(t *testing.T) {
	// HighBits/LowBits laid out big-endian across the 16 bytes.
	u := uuidFromWit(types.Uuid{HighBits: 0x0011223344556677, LowBits: 0x8899aabbccddeeff})
	want := "00112233-4455-6677-8899-aabbccddeeff"
	if u.String() != want {
		t.Fatalf("uuid = %s, want %s", u.String(), want)
	}
}
