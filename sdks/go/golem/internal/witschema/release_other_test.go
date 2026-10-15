//go:build !wasip1

package witschema

import (
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	"testing"
)

func TestReleaseRawTreeIncludesUnreachableCapabilities(t *testing.T) {
	original := releaseNode
	defer func() { releaseNode = original }()
	var tags []uint8
	releaseNode = func(n types.SchemaValueNode) { tags = append(tags, n.Tag()) }
	ReleaseTree(types.SchemaValueTree{Root: 100, ValueNodes: []types.SchemaValueNode{
		types.MakeSchemaValueNodeStringValue("plain"),
		types.MakeSchemaValueNodeSecretValue(nil),
		types.MakeSchemaValueNodeQuotaTokenHandle(nil),
		types.MakeSchemaValueNodePermissionCardHandle(nil),
		types.MakeSchemaValueNodeStreamValue(nil),
	}})
	want := []uint8{types.SchemaValueNodeSecretValue, types.SchemaValueNodeQuotaTokenHandle, types.SchemaValueNodePermissionCardHandle, types.SchemaValueNodeStreamValue}
	if len(tags) != len(want) {
		t.Fatalf("released %v", tags)
	}
	for i := range want {
		if tags[i] != want[i] {
			t.Fatalf("released %v", tags)
		}
	}
}
