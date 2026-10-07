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

package engine

import (
	"fmt"
	"reflect"
	"slices"

	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// Unstructured text and binary values are a variant of an inline case and a
// url case, marked with their role; a multimodal list is a list of a variant,
// marked with the multimodal role. These are the same shapes every SDK emits.

func roleMetadata(role types.Role) *types.MetadataEnvelope {
	return &types.MetadataEnvelope{
		Doc:        witTypes.None[string](),
		Deprecated: witTypes.None[string](),
		Role:       witTypes.Some(role),
	}
}

func emptyMetadata() types.MetadataEnvelope {
	return types.MetadataEnvelope{Doc: witTypes.None[string](), Deprecated: witTypes.None[string]()}
}

// Push adds an anonymous node and returns its index.
func (g *GraphBuilder) Push(body types.SchemaTypeBody) int32 {
	g.Nodes = append(g.Nodes, types.SchemaTypeNode{Body: body, Metadata: emptyMetadata()})
	return int32(len(g.Nodes) - 1)
}

func listOrNone(items []string) witTypes.Option[[]string] {
	if len(items) == 0 {
		return witTypes.None[[]string]()
	}
	return witTypes.Some(slices.Clone(items))
}

// inlineOrURL is the variant both unstructured kinds share: case 0 carries the
// content inline, case 1 refers to it by URL.
func inlineOrURL(g *GraphBuilder, inline types.SchemaTypeBody) types.SchemaTypeBody {
	return types.MakeSchemaTypeBodyVariantType([]types.VariantCaseType{
		{Name: "inline", Payload: witTypes.Some(g.Push(inline)), Metadata: emptyMetadata()},
		{Name: "url", Payload: witTypes.Some(g.Push(types.MakeSchemaTypeBodyUrlType(types.UrlRestrictions{
			AllowedSchemes: witTypes.None[[]string](),
			AllowedHosts:   witTypes.None[[]string](),
		}))), Metadata: emptyMetadata()},
	})
}

func pushVariant(b *ValBuilder, which uint32, payload int32) int32 {
	return b.Push(types.MakeSchemaValueNodeVariantValue(types.VariantValuePayload{Case: which, Payload: witTypes.Some(payload)}))
}

// inlineOrURLPayload reads the case and payload node of an unstructured value.
func inlineOrURLPayload(d *Decoder, c *Codec, idx int32) (uint32, types.SchemaValueNode, error) {
	n, err := d.Node(idx)
	if err != nil {
		return 0, types.SchemaValueNode{}, err
	}
	if n.Tag() != types.SchemaValueNodeVariantValue {
		return 0, types.SchemaValueNode{}, fmt.Errorf("cannot decode value node (tag %d) into %s", n.Tag(), c.Typ)
	}
	v := n.VariantValue()
	if v.Payload.IsNone() {
		return 0, types.SchemaValueNode{}, fmt.Errorf("%s: case %d carries no payload", c.Typ, v.Case)
	}
	payload, err := d.Node(v.Payload.Some())
	return v.Case, payload, err
}

func compileUnstructuredText(c *Codec, languages []string) {
	c.Metadata = roleMetadata(types.MakeRoleUnstructuredText())
	c.Body = func(g *GraphBuilder) types.SchemaTypeBody {
		return inlineOrURL(g, types.MakeSchemaTypeBodyTextType(types.TextRestrictions{
			Languages: listOrNone(languages),
			MinLength: witTypes.None[uint32](),
			MaxLength: witTypes.None[uint32](),
			Regex:     witTypes.None[string](),
		}))
	}
	c.Encode = func(b *ValBuilder, v reflect.Value) int32 {
		if url := v.FieldByName("URL").String(); url != "" {
			return pushVariant(b, 1, b.Push(types.MakeSchemaValueNodeUrlValue(url)))
		}
		language := witTypes.None[string]()
		if l := v.FieldByName("Language").String(); l != "" {
			language = witTypes.Some(l)
		}
		return pushVariant(b, 0, b.Push(types.MakeSchemaValueNodeTextValue(types.TextValuePayload{
			Text:     v.FieldByName("Text").String(),
			Language: language,
		})))
	}
	c.Decode = func(d *Decoder, dst reflect.Value, idx int32) error {
		which, payload, err := inlineOrURLPayload(d, c, idx)
		if err != nil {
			return err
		}
		switch {
		case which == 0 && payload.Tag() == types.SchemaValueNodeTextValue:
			text := payload.TextValue()
			dst.FieldByName("Text").SetString(text.Text)
			if text.Language.IsSome() {
				l := text.Language.Some()
				if len(languages) > 0 && !slices.Contains(languages, l) {
					return fmt.Errorf("%s: language %q is not accepted", c.Typ, l)
				}
				dst.FieldByName("Language").SetString(l)
			}
		case which == 1 && payload.Tag() == types.SchemaValueNodeUrlValue:
			dst.FieldByName("URL").SetString(payload.UrlValue())
		default:
			return fmt.Errorf("%s: case %d has an unexpected payload (tag %d)", c.Typ, which, payload.Tag())
		}
		return nil
	}
}

func compileUnstructuredBinary(c *Codec, mimeTypes []string) {
	c.Metadata = roleMetadata(types.MakeRoleUnstructuredBinary())
	c.Body = func(g *GraphBuilder) types.SchemaTypeBody {
		return inlineOrURL(g, types.MakeSchemaTypeBodyBinaryType(types.BinaryRestrictions{
			MimeTypes: listOrNone(mimeTypes),
			MinBytes:  witTypes.None[uint32](),
			MaxBytes:  witTypes.None[uint32](),
		}))
	}
	c.Encode = func(b *ValBuilder, v reflect.Value) int32 {
		if url := v.FieldByName("URL").String(); url != "" {
			return pushVariant(b, 1, b.Push(types.MakeSchemaValueNodeUrlValue(url)))
		}
		mime := witTypes.None[string]()
		if m := v.FieldByName("MimeType").String(); m != "" {
			mime = witTypes.Some(m)
		}
		return pushVariant(b, 0, b.Push(types.MakeSchemaValueNodeBinaryValue(types.BinaryValuePayload{
			Bytes:    append([]uint8(nil), v.FieldByName("Data").Bytes()...),
			MimeType: mime,
		})))
	}
	c.Decode = func(d *Decoder, dst reflect.Value, idx int32) error {
		which, payload, err := inlineOrURLPayload(d, c, idx)
		if err != nil {
			return err
		}
		switch {
		case which == 0 && payload.Tag() == types.SchemaValueNodeBinaryValue:
			data := payload.BinaryValue()
			dst.FieldByName("Data").SetBytes(append([]byte(nil), data.Bytes...))
			if data.MimeType.IsSome() {
				m := data.MimeType.Some()
				if len(mimeTypes) > 0 && !slices.Contains(mimeTypes, m) {
					return fmt.Errorf("%s: media type %q is not accepted", c.Typ, m)
				}
				dst.FieldByName("MimeType").SetString(m)
			}
		case which == 1 && payload.Tag() == types.SchemaValueNodeUrlValue:
			dst.FieldByName("URL").SetString(payload.UrlValue())
		default:
			return fmt.Errorf("%s: case %d has an unexpected payload (tag %d)", c.Typ, which, payload.Tag())
		}
		return nil
	}
}

// compileMultimodal is a list of items whose type is a registered variant,
// marked with the multimodal role.
func (d *Engine) compileMultimodal(c *Codec, elem reflect.Type) {
	if _, ok := d.Variants[elem]; !ok {
		MarkInvalid(c, "%s holds %s, which is not a registered variant; declare its modalities with golem.DefineVariant", c.Typ, elem)
		return
	}
	compileList(c, d.Compile(elem))
	c.Metadata = roleMetadata(types.MakeRoleMultimodal())
}
