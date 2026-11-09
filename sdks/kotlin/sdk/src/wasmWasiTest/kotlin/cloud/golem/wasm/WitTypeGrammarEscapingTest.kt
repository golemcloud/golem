package cloud.golem.wasm

import kotlin.test.Test
import kotlin.test.assertEquals

/**
 * The hex encoding that lets `path`, `quantity` and `union`'s discriminator carry arbitrary
 * free-form strings on the lowered path -- a mime type, a regex, a literal -- without ever
 * containing a `,` / `<` / `>` that the grammar would mistake for structure. The same convention
 * `cloud.golem.runtime.ToolRefinement` already uses for a `text` type's `regex` refinement,
 * reused here rather than duplicated. Hex output is only `[0-9a-f]`, so the shared
 * `splitTopLevelCommas` needs no change to carry it.
 */
class WitTypeGrammarEscapingTest {

    @Test
    fun aPayloadWithACommaSurvivesHexEncoding() {
        val encoded = escapeGrammarLeaf("a,b")
        assertEquals("612c62", encoded, "hex of the UTF-8 bytes 0x61 0x2c 0x62")
        assertEquals("a,b", unescapeGrammarLeaf(encoded))
    }

    @Test
    fun aPayloadWithAngleBracketsSurvivesHexEncoding() {
        val encoded = escapeGrammarLeaf("<tag>")
        assertEquals("<tag>", unescapeGrammarLeaf(encoded))
        assertEquals(true, encoded.none { it == '<' || it == '>' || it == ',' }, "hex output carries no grammar character")
    }

    @Test
    fun hexEncodingMatchesToolRefinementsExistingRegexConvention() {
        // The exact worked example from ToolRefinement.kt's own doc comment: a regex whose hex
        // encoding is shown there is reproduced bit-for-bit by this function.
        assertEquals("5e5b612d7a5d2b24", escapeGrammarLeaf("^[a-z]+$"))
    }

    @Test
    fun emptyStringEncodesToEmptyHex() {
        assertEquals("", escapeGrammarLeaf(""))
        assertEquals("", unescapeGrammarLeaf(""))
    }

    @Test
    fun existingCompositesStillSplitExactlyAsBefore() {
        // The split function and its callers are untouched by this change; this is a plain
        // regression check that nothing here disturbed them.
        assertEquals(listOf("x:s32", "y:string"), recordFields("record<x:s32,y:string>").map { "${it.first}:${it.second}" })
        assertEquals(listOf("a", "b", "c"), splitTopLevelCommas("a,b,c"))
        assertEquals(listOf("list<s32>", "string"), splitTopLevelCommas("list<s32>,string"))
    }

    @Test
    fun pathSpecRoundTripsWithHexEncodedAllowLists() {
        val wit = "path<input,file,${optionalEscapedLeafList(listOf("image/png", "a,b<c>"))},_>"
        val parsed = parsePathSpec(wit)
        assertEquals("input", parsed.direction)
        assertEquals("file", parsed.kind)
        assertEquals(listOf("image/png", "a,b<c>"), parsed.allowedMimeTypes)
        assertEquals(null, parsed.allowedExtensions)
    }

    @Test
    fun quantitySpecRoundTripsWithAHexEncodedUnitAndBounds() {
        val unit = "m/s,<rate>"
        val wit = "quantity<${escapeGrammarLeaf(unit)},${escapedLeafList(listOf("km/h"))},<0,0,${escapeGrammarLeaf(unit)}>,_>"
        val parsed = parseQuantitySpec(wit)
        assertEquals(unit, parsed.baseUnit)
        assertEquals(listOf("km/h"), parsed.allowedSuffixes)
        assertEquals(ParsedQuantityValue(0, 0, unit), parsed.min)
        assertEquals(null, parsed.max)
    }

    @Test
    fun everyDiscriminatorRuleRoundTripsWithHexEncodedPayloads() {
        assertEquals(ParsedDiscriminatorRule.Prefix("a,b"), parseDiscriminatorRule("prefix<${escapeGrammarLeaf("a,b")}>"))
        assertEquals(ParsedDiscriminatorRule.Suffix(".tar.gz"), parseDiscriminatorRule("suffix<${escapeGrammarLeaf(".tar.gz")}>"))
        assertEquals(ParsedDiscriminatorRule.Contains("x"), parseDiscriminatorRule("contains<${escapeGrammarLeaf("x")}>"))
        val regex = "^(a,b)<c>$"
        assertEquals(ParsedDiscriminatorRule.Regex(regex), parseDiscriminatorRule("regex<${escapeGrammarLeaf(regex)}>"))
        assertEquals(
            ParsedDiscriminatorRule.FieldEquals("kind", "circle"),
            parseDiscriminatorRule("field-equals<${escapeGrammarLeaf("kind")},${escapeGrammarLeaf("circle")}>"),
        )
        assertEquals(
            ParsedDiscriminatorRule.FieldEquals("kind", null),
            parseDiscriminatorRule("field-equals<${escapeGrammarLeaf("kind")},_>"),
        )
        assertEquals(ParsedDiscriminatorRule.FieldAbsent("kind"), parseDiscriminatorRule("field-absent<${escapeGrammarLeaf("kind")}>"))
    }

    @Test
    fun unionBranchesRoundTripIncludingARecursiveTypeAndAHexEncodedTag() {
        val wit = "union<<${escapeGrammarLeaf("a,b")},s32,prefix<${escapeGrammarLeaf("x<y")}>>," +
            "<${escapeGrammarLeaf("c")},list<string>,field-absent<${escapeGrammarLeaf("kind")}>>>"
        val branches = loweredUnionBranches(wit)
        assertEquals(2, branches.size, "wit was: $wit")
        assertEquals("a,b", branches[0].tag, "branch0=${branches[0]}")
        assertEquals("s32", branches[0].type, "branch0=${branches[0]}")
        assertEquals(ParsedDiscriminatorRule.Prefix("x<y"), parseDiscriminatorRule(branches[0].rule), "branch0=${branches[0]}")
        assertEquals("c", branches[1].tag, "branch1=${branches[1]}")
        assertEquals("list<string>", branches[1].type, "branch1=${branches[1]}")
        assertEquals(ParsedDiscriminatorRule.FieldAbsent("kind"), parseDiscriminatorRule(branches[1].rule), "branch1=${branches[1]}")
    }
}
