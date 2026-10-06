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
	"slices"
	"strings"
	"testing"

	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
)

type enOrDe struct{}

func (enOrDe) Languages() []string { return []string{"en", "de"} }

type images struct{}

func (images) MimeTypes() []string { return []string{"image/png"} }

// A custom modality set: a caption or a chart.
type Figure interface{ isFigure() }

type Caption struct{ Value UnstructuredText[AnyLanguage] }
type Chart struct{ Points []int32 }

func (Caption) isFigure() {}
func (Chart) isFigure()   {}

var _ = DefineVariant[Figure](WrappedCase[Caption]("Caption"), Case[Chart]("Chart"))

type Content struct {
	Summary UnstructuredText[enOrDe]
	Photo   UnstructuredBinary[images]
	Media   Multimodal
	Figures MultimodalOf[Figure]
}

func TestUnstructuredValuesRoundTrip(t *testing.T) {
	assertRoundTrip(t, "inline", Content{
		Summary: UnstructuredText[enOrDe]{Text: "hallo", Language: "de"},
		Photo:   UnstructuredBinary[images]{Data: []byte{1, 2}, MimeType: "image/png"},
		Media: Multimodal{
			TextModality{Value: UnstructuredText[AnyLanguage]{Text: "hi"}},
			BinaryModality{Value: UnstructuredBinary[AnyMimeType]{URL: "https://example.com/a.bin"}},
		},
		Figures: MultimodalOf[Figure]{Caption{Value: UnstructuredText[AnyLanguage]{URL: "https://example.com/c.txt"}}, Chart{Points: []int32{1, 2}}},
	})
	assertRoundTrip(t, "by url", Content{
		Summary: UnstructuredText[enOrDe]{URL: "https://example.com/s.txt"},
		Photo:   UnstructuredBinary[images]{URL: "https://example.com/p.png"},
		Media:   Multimodal{},
		Figures: MultimodalOf[Figure]{},
	})
}

// The shapes are the cross-SDK conventions: a role-marked variant of inline
// and url, and a role-marked list of a variant.
func TestUnstructuredValuesPublishTheirRoles(t *testing.T) {
	g := graphBuilder{d: defs}
	root := g.node(defs.compile(reflect.TypeFor[Content]()))
	graph := g.build()
	fields := graph.TypeNodes[root].Body.RecordType()
	node := func(i int) types.SchemaTypeNode { return graph.TypeNodes[fields[i].Body] }

	summary := node(0)
	if summary.Metadata.Role.IsNone() || summary.Metadata.Role.Some().Tag() != types.RoleUnstructuredText {
		t.Fatalf("summary has no unstructured-text role: %+v", summary.Metadata)
	}
	cases := summary.Body.VariantType()
	if cases[0].Name != "inline" || cases[1].Name != "url" {
		t.Fatalf("summary cases %+v", cases)
	}
	inline := graph.TypeNodes[cases[0].Payload.Some()].Body.TextType()
	if !slices.Equal(inline.Languages.Some(), []string{"en", "de"}) {
		t.Fatalf("summary languages %+v", inline)
	}
	if photo := node(1); photo.Metadata.Role.Some().Tag() != types.RoleUnstructuredBinary {
		t.Fatalf("photo role %+v", photo.Metadata)
	}
	media := node(2)
	if media.Metadata.Role.IsNone() || media.Metadata.Role.Some().Tag() != types.RoleMultimodal {
		t.Fatalf("media has no multimodal role: %+v", media.Metadata)
	}
	modalities := graph.TypeNodes[media.Body.ListType()].Body.VariantType()
	if modalities[0].Name != "Text" || modalities[1].Name != "Binary" {
		t.Fatalf("basic modalities %+v", modalities)
	}
	if figures := graph.TypeNodes[node(3).Body.ListType()].Body.VariantType(); figures[0].Name != "Caption" {
		t.Fatalf("custom modalities %+v", figures)
	}
}

func TestAMultimodalListNeedsARegisteredVariant(t *testing.T) {
	c := defs.compile(reflect.TypeFor[MultimodalOf[string]]())
	if !strings.Contains(c.invalid, "not a registered variant") {
		t.Fatalf("invalid = %q", c.invalid)
	}
}
