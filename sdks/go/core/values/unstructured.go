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

package values

import "reflect"

// Unstructured and multimodal values.
//
// These are the conventions agents use for content a model or a person reads:
// text or binary data carried inline or referred to by URL, and lists that mix
// several such modalities. Each is an ordinary schema shape — a variant, or a
// list of one — marked with a role so tools and UIs can treat it as content.

// Languages lists the BCP-47 languages an [UnstructuredText] accepts. Implement
// it on a marker type; the list is read from the type parameter's zero value.
//
//	type EnOrDe struct{}
//
//	func (EnOrDe) Languages() []string { return []string{"en", "de"} }
type Languages interface {
	Languages() []string
}

// AnyLanguage accepts text in any language.
type AnyLanguage struct{}

func (AnyLanguage) Languages() []string { return nil }

// UnstructuredText is text carried inline, or referred to by URL. A non-empty
// URL is the reference; otherwise Text is the content, in Language when it is
// set.
type UnstructuredText[L Languages] struct {
	URL      string
	Text     string
	Language string
}

func (UnstructuredText[L]) unstructuredLanguages() []string {
	var l L
	return l.Languages()
}

// MimeTypes lists the media types an [UnstructuredBinary] accepts. Implement it
// on a marker type, as [Languages].
type MimeTypes interface {
	MimeTypes() []string
}

// AnyMimeType accepts data of any media type.
type AnyMimeType struct{}

func (AnyMimeType) MimeTypes() []string { return nil }

// UnstructuredBinary is data carried inline, or referred to by URL. A
// non-empty URL is the reference; otherwise Data is the content, of MimeType.
type UnstructuredBinary[M MimeTypes] struct {
	URL      string
	Data     []byte
	MimeType string
}

func (UnstructuredBinary[M]) unstructuredMimeTypes() []string {
	var m M
	return m.MimeTypes()
}

// Modality is one item of a [Multimodal] list: a [TextModality] or a
// [BinaryModality].
type Modality interface{ isModality() }

// TextModality is a text item of a [Multimodal] list.
type TextModality struct{ Value UnstructuredText[AnyLanguage] }

// BinaryModality is a binary item of a [Multimodal] list.
type BinaryModality struct {
	Value UnstructuredBinary[AnyMimeType]
}

func (TextModality) isModality()   {}
func (BinaryModality) isModality() {}

// MultimodalOf is a list mixing the modalities of T, a variant whose cases are
// the kinds of content the list may hold.
type MultimodalOf[T any] []T

func (MultimodalOf[T]) multimodalElem() reflect.Type { return reflect.TypeFor[T]() }

// Multimodal is a list of text and binary items.
type Multimodal = MultimodalOf[Modality]

// UnstructuredTextLanguages reports the languages an UnstructuredText accepts,
// nil for any.
func UnstructuredTextLanguages(v any) ([]string, bool) {
	u, ok := v.(interface{ unstructuredLanguages() []string })
	if !ok {
		return nil, false
	}
	return u.unstructuredLanguages(), true
}

// UnstructuredBinaryMimeTypes reports the media types an UnstructuredBinary
// accepts, nil for any.
func UnstructuredBinaryMimeTypes(v any) ([]string, bool) {
	u, ok := v.(interface{ unstructuredMimeTypes() []string })
	if !ok {
		return nil, false
	}
	return u.unstructuredMimeTypes(), true
}

// MultimodalElem reports the item type of a MultimodalOf.
func MultimodalElem(v any) (reflect.Type, bool) {
	m, ok := v.(interface{ multimodalElem() reflect.Type })
	if !ok {
		return nil, false
	}
	return m.multimodalElem(), true
}
