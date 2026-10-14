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

package schema

import (
	"fmt"
	"math"
	"math/big"
	"net/url"
	"regexp"
	"slices"
	"strings"
	"unicode/utf8"
)

// ConstraintViolationError reports well-formed JSON whose value breaks one of
// its type's restrictions: a bound, a length, an allowed language, MIME type,
// extension, scheme, host or unit.
type ConstraintViolationError struct {
	// Path locates the value, as in other packing errors; empty is the root.
	Path   string
	Reason string
}

func (e *ConstraintViolationError) Error() string {
	return fmt.Sprintf("%s: %s", pathOrRoot(e.Path), e.Reason)
}

func violation(path, format string, args ...any) error {
	return &ConstraintViolationError{Path: path, Reason: fmt.Sprintf(format, args...)}
}

// checkNumeric enforces the bounds of a numeric restriction. The value and the
// bounds may be of different families, so they are compared exactly as
// rationals; a NaN is outside every bound.
func checkNumeric(rs *NumericRestrictions, value NumericBound, path string) error {
	if rs == nil {
		return nil
	}
	v, ok := boundRat(value)
	if rs.Min != nil {
		if min, minOK := boundRat(*rs.Min); minOK && (!ok || v.Cmp(min) < 0) {
			return violation(path, "below minimum %s", describeBound(*rs.Min))
		}
	}
	if rs.Max != nil {
		if max, maxOK := boundRat(*rs.Max); maxOK && (!ok || v.Cmp(max) > 0) {
			return violation(path, "above maximum %s", describeBound(*rs.Max))
		}
	}
	return nil
}

func boundRat(b NumericBound) (*big.Rat, bool) {
	switch b.Kind {
	case BoundSigned:
		return new(big.Rat).SetInt64(b.Signed), true
	case BoundUnsigned:
		return new(big.Rat).SetInt(new(big.Int).SetUint64(b.Unsigned)), true
	default:
		f := math.Float64frombits(b.FloatBits)
		if math.IsNaN(f) || math.IsInf(f, 0) {
			return nil, false
		}
		return new(big.Rat).SetFloat64(f), true
	}
}

func describeBound(b NumericBound) string {
	switch b.Kind {
	case BoundSigned:
		return fmt.Sprint(b.Signed)
	case BoundUnsigned:
		return fmt.Sprint(b.Unsigned)
	default:
		return fmt.Sprint(math.Float64frombits(b.FloatBits))
	}
}

func signedBound(n int64) NumericBound    { return NumericBound{Kind: BoundSigned, Signed: n} }
func unsignedBound(n uint64) NumericBound { return NumericBound{Kind: BoundUnsigned, Unsigned: n} }
func floatBound(f float64) NumericBound {
	return NumericBound{Kind: BoundFloatBits, FloatBits: math.Float64bits(f)}
}

func checkText(rs TextRestrictions, v TextValue, path string) error {
	if rs.Languages != nil && v.Language != nil && !slices.Contains(*rs.Languages, *v.Language) {
		return violation(path, "language %q is not allowed", *v.Language)
	}
	n := utf8.RuneCountInString(v.Text)
	if rs.MinLength != nil && n < int(*rs.MinLength) {
		return violation(path, "text is %d characters, shorter than %d", n, *rs.MinLength)
	}
	if rs.MaxLength != nil && n > int(*rs.MaxLength) {
		return violation(path, "text is %d characters, longer than %d", n, *rs.MaxLength)
	}
	if rs.Regex != nil {
		// A pattern Go cannot compile is a schema problem, reported elsewhere.
		if re, err := regexp.Compile(*rs.Regex); err == nil && !re.MatchString(v.Text) {
			return violation(path, "text does not match %q", *rs.Regex)
		}
	}
	return nil
}

func checkBinary(rs BinaryRestrictions, v BinaryValue, path string) error {
	if rs.MimeTypes != nil && v.MimeType != nil && !slices.Contains(*rs.MimeTypes, *v.MimeType) {
		return violation(path, "MIME type %q is not allowed", *v.MimeType)
	}
	n := len(v.Bytes)
	if rs.MinBytes != nil && n < int(*rs.MinBytes) {
		return violation(path, "%d bytes, fewer than %d", n, *rs.MinBytes)
	}
	if rs.MaxBytes != nil && n > int(*rs.MaxBytes) {
		return violation(path, "%d bytes, more than %d", n, *rs.MaxBytes)
	}
	return nil
}

// checkPath enforces what a path value can show on its own: that it is not
// empty and, when it has one, its extension. MIME restrictions need content
// and are left to whoever reads the file.
func checkPath(spec PathSpec, p string, path string) error {
	if p == "" {
		return violation(path, "path is empty")
	}
	if spec.AllowedExtensions == nil {
		return nil
	}
	name := p[strings.LastIndexByte(p, '/')+1:]
	dot := strings.LastIndexByte(name, '.')
	if dot < 0 || dot == len(name)-1 {
		return nil
	}
	if ext := name[dot+1:]; !slices.Contains(*spec.AllowedExtensions, ext) {
		return violation(path, "extension %q is not allowed", ext)
	}
	return nil
}

func checkURL(rs UrlRestrictions, raw string, path string) error {
	if raw == "" {
		return violation(path, "URL is empty")
	}
	u, err := url.Parse(raw)
	if err != nil || u.Scheme == "" {
		return violation(path, "%q is not an absolute URL", raw)
	}
	if rs.AllowedSchemes != nil && !containsFold(*rs.AllowedSchemes, u.Scheme) {
		return violation(path, "scheme %q is not allowed", u.Scheme)
	}
	if rs.AllowedHosts != nil {
		host := u.Hostname()
		if host == "" {
			return violation(path, "URL has no host")
		}
		if !containsFold(*rs.AllowedHosts, host) {
			return violation(path, "host %q is not allowed", host)
		}
	}
	return nil
}

func containsFold(list []string, s string) bool {
	return slices.ContainsFunc(list, func(item string) bool { return strings.EqualFold(item, s) })
}

// checkQuantity enforces the unit, then the range. Without allowed suffixes
// only the base unit is accepted; bounds compare at a common scale.
func checkQuantity(spec QuantitySpec, v QuantityValue, path string) error {
	unitOK := v.Unit == spec.BaseUnit
	if len(spec.AllowedSuffixes) > 0 {
		unitOK = slices.Contains(spec.AllowedSuffixes, v.Unit)
	}
	if !unitOK {
		return violation(path, "unit %q is not allowed", v.Unit)
	}
	if spec.Min != nil && quantityRat(v).Cmp(quantityRat(*spec.Min)) < 0 {
		return violation(path, "below minimum %s", renderQuantity(*spec.Min))
	}
	if spec.Max != nil && quantityRat(v).Cmp(quantityRat(*spec.Max)) > 0 {
		return violation(path, "above maximum %s", renderQuantity(*spec.Max))
	}
	return nil
}

func quantityRat(q QuantityValue) *big.Rat {
	scale := new(big.Int).Exp(big.NewInt(10), big.NewInt(int64(abs32(q.Scale))), nil)
	r := new(big.Rat).SetInt64(q.Mantissa)
	if q.Scale >= 0 {
		return r.Quo(r, new(big.Rat).SetInt(scale))
	}
	return r.Mul(r, new(big.Rat).SetInt(scale))
}

func abs32(n int32) int64 {
	if n < 0 {
		return -int64(n)
	}
	return int64(n)
}
