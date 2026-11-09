#!/usr/bin/env python3
"""Generates sdk/src/wasmWasiMain/kotlin/cloud/golem/runtime/host/OplogEntries.kt from the WIT.

`golem:api/oplog`'s `public-oplog-entry` is a ~50-case variant that grows with almost every Golem
release (1.6.0 added a dozen cases and changed most of the old ones). Its canonical-ABI layout
(sizes, alignments, field offsets, variant payload offsets) and its lift code are therefore derived
from the resolved WIT rather than written by hand. Re-run after scripts/sync-wit.sh.

Types that already have a hand-written Kotlin counterpart (uuid, agent-id, typed-schema-value, the
retry policy, span attributes, ...) are mapped onto it (OVERRIDES); their *layout* still comes from the
WIT so the surrounding offsets stay right.

Usage: scripts/gen-oplog.py [--check]     (needs `wasm-tools` on PATH)
"""
import json
import re
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent.parent
OUT = HERE / "sdk/src/wasmWasiMain/kotlin/cloud/golem/runtime/host/OplogEntries.kt"
WIT_DIR = str(HERE / "wit-native")
if "--wit-dir" in sys.argv:  # layout self-test against another WIT revision (output goes to stdout)
    WIT_DIR = sys.argv[sys.argv.index("--wit-dir") + 1]
ROOT_NAME = "public-oplog-entry"
ROOT_INTERFACE = "oplog"

KEYWORDS = {
    "as", "break", "class", "continue", "do", "else", "false", "for", "fun", "if", "in", "interface",
    "is", "null", "object", "package", "return", "super", "this", "throw", "true", "try", "typealias",
    "typeof", "val", "var", "when", "while",
}

# WIT type name -> (kotlin type, lift expression template with {a} = address). Layout still from WIT.
OVERRIDES = {
    "typed-schema-value": ("TypedSchemaValue", "liftTypedSchemaValue({a})"),
    "uuid": ("Uuid", "Uuid(loadLong({a}), loadLong({a} + 8))"),
    "agent-id": ("AgentId", "AgentId(ComponentId(Uuid(loadLong({a}), loadLong({a} + 8))), liftString(loadInt({a} + 16), loadInt({a} + 20)))"),
    "component-id": ("ComponentId", "ComponentId(Uuid(loadLong({a}), loadLong({a} + 8)))"),
    "environment-id": ("EnvironmentId", "EnvironmentId(Uuid(loadLong({a}), loadLong({a} + 8)))"),
    "instant": ("Timestamp", "Timestamp(loadLong({a}), loadInt({a} + 8))"),
    "named-retry-policy": ("NamedRetryPolicy", "liftNamedRetryPolicy({a})"),
    # golem:api/context's span attributes (ContextApi.kt). attribute-value has a single `string` case.
    "attribute-value": ("AttributeValue", "AttributeValue.StringValue(liftString(loadInt({a} + 4), loadInt({a} + 8)))"),
    "attribute": ("Attribute", "Attribute(liftString(loadInt({a}), loadInt({a} + 4)), AttributeValue.StringValue(liftString(loadInt({a} + 12), loadInt({a} + 16))))"),
}
# `datetime` is an alias of `instant` in this interface; both resolve to the same record.
ALIAS_OVERRIDE_NAMES = {"datetime": "instant"}


def die(msg):
    sys.exit(f"gen-oplog: {msg}")


def load_json():
    try:
        out = subprocess.run(
            ["wasm-tools", "component", "wit", WIT_DIR, "--json"],
            check=True, capture_output=True, text=True,
        ).stdout
    except (OSError, subprocess.CalledProcessError) as e:
        die(f"could not run `wasm-tools component wit --json`: {e}")
    return json.loads(out)


def align_to(n, a):
    return (n + a - 1) // a * a


def pascal(name):
    return "".join(p[:1].upper() + p[1:] for p in name.replace("%", "").split("-"))


def camel(name):
    p = pascal(name)
    n = p[:1].lower() + p[1:]
    return f"`{n}`" if n in KEYWORDS else n


def screaming(name):
    return name.replace("%", "").replace("-", "_").upper()


class Gen:
    def __init__(self, j):
        self.types = j["types"]
        self.ifaces = j["interfaces"]
        self.decls = {}       # kotlin name -> declaration text
        self.lifts = {}       # kotlin name -> lift function text
        self.order = []
        self._layout = {}

    # ---- type graph helpers -------------------------------------------------------------------
    def tdef(self, t):
        return self.types[t]

    def kind(self, t):
        k = self.tdef(t)["kind"]
        if isinstance(k, str):
            return k, None
        (kk, v), = k.items()
        return kk, v

    def resolve(self, t):
        """Follows aliases, stopping at a type that has an override."""
        while not isinstance(t, str):
            name = self.tdef(t).get("name")
            if self.override_key(t):
                return t
            kk, v = self.kind(t)
            if kk != "type":
                return t
            t = v
        return t

    def override_key(self, t):
        if isinstance(t, str):
            return None
        d = self.tdef(t)
        name = d.get("name")
        name = ALIAS_OVERRIDE_NAMES.get(name, name)
        if name in OVERRIDES and d.get("owner") is not None:
            return name
        return None

    # ---- canonical ABI layout -----------------------------------------------------------------
    PRIM = {"bool": (1, 1), "u8": (1, 1), "s8": (1, 1), "u16": (2, 2), "s16": (2, 2), "u32": (4, 4),
            "s32": (4, 4), "char": (4, 4), "f32": (4, 4), "u64": (8, 8), "s64": (8, 8), "f64": (8, 8),
            "string": (8, 4)}

    def layout(self, t):
        t = self.resolve(t)
        if isinstance(t, str):
            return self.PRIM[t]
        if t in self._layout:
            return self._layout[t]
        kk, v = self.kind(t)
        while kk == "type":  # alias of an overridden type: the layout is that of its target
            if isinstance(v, str):
                return self.PRIM[v]
            kk, v = self.kind(v)
        if kk == "record":
            res = self.record_layout([f["type"] for f in v["fields"]])[:2]
        elif kk == "tuple":
            res = self.record_layout(v["types"])[:2]
        elif kk == "list":
            res = (8, 4)
        elif kk == "enum":
            sz = self.tag_size(len(v["cases"]))
            res = (sz, sz)
        elif kk == "option":
            res = self.variant_layout([None, v])[:2]
        elif kk == "result":
            res = self.variant_layout([v.get("ok"), v.get("err")])[:2]
        elif kk == "variant":
            res = self.variant_layout([c.get("type") for c in v["cases"]])[:2]
        elif kk == "handle":
            res = (4, 4)
        else:
            die(f"unsupported type kind {kk}")
        self._layout[t] = res
        return res

    def record_layout(self, field_types):
        off, al, offs = 0, 1, []
        for ft in field_types:
            s, a = self.layout(ft)
            off = align_to(off, a)
            offs.append(off)
            off += s
            al = max(al, a)
        return align_to(off, al), al, offs

    @staticmethod
    def tag_size(n):
        return 1 if n <= 256 else 2 if n <= 65536 else 4

    def variant_layout(self, payload_types):
        tag = self.tag_size(len(payload_types))
        al = tag
        mx = 0
        for p in payload_types:
            if p is not None:
                s, a = self.layout(p)
                al = max(al, a)
                mx = max(mx, s)
        po = align_to(tag, max([1] + [self.layout(p)[1] for p in payload_types if p is not None]))
        return align_to(po + mx, al), al, po

    # ---- Kotlin type + lift -------------------------------------------------------------------
    def ktype(self, t):
        t = self.resolve(t)
        if isinstance(t, str):
            return {"bool": "Boolean", "u8": "Int", "s8": "Int", "u16": "Int", "s16": "Int", "u32": "UInt",
                    "s32": "Int", "u64": "Long", "s64": "Long", "f32": "Float", "f64": "Double",
                    "char": "Int", "string": "String"}[t]
        ov = self.override_key(t)
        if ov:
            return OVERRIDES[ov][0]
        kk, v = self.kind(t)
        if kk in ("record", "variant", "enum"):
            return self.named(t)
        if kk == "option":
            inner = self.ktype(v)
            if inner.endswith("?"):
                die("nested option is not supported")
            return inner + "?"
        if kk == "list":
            return "ByteArray" if self.resolve(v) == "u8" else f"List<{self.ktype(v)}>"
        if kk == "tuple":
            ts = [self.ktype(x) for x in v["types"]]
            if len(ts) == 2:
                return f"Pair<{ts[0]}, {ts[1]}>"
            if len(ts) == 3:
                return f"Triple<{ts[0]}, {ts[1]}, {ts[2]}>"
            die("tuples larger than 3 are not supported")
        if kk == "result":
            ok = "Unit" if v.get("ok") is None else self.ktype(v["ok"])
            err = "Unit" if v.get("err") is None else self.ktype(v["err"])
            return f"Either<{err}, {ok}>"
        die(f"no Kotlin type for {kk}")

    def lift(self, t, a, depth=0):
        """Kotlin expression lifting type `t` stored at address expression `a`."""
        t = self.resolve(t)
        if isinstance(t, str):
            return {
                "bool": f"(loadByte({a}).toInt() != 0)",
                "u8": f"(loadByte({a}).toInt() and 0xFF)",
                "s8": f"loadByte({a}).toInt()",
                "u16": f"(loadShort({a}).toInt() and 0xFFFF)",
                "s16": f"loadShort({a}).toInt()",
                "u32": f"loadInt({a}).toUInt()",
                "s32": f"loadInt({a})",
                "u64": f"loadLong({a})",
                "s64": f"loadLong({a})",
                "f32": f"loadFloat({a})",
                "f64": f"loadDouble({a})",
                "char": f"loadInt({a})",
                "string": f"liftString(loadInt({a}), loadInt(({a}) + 4))",
            }[t]
        ov = self.override_key(t)
        if ov:
            return OVERRIDES[ov][1].replace("{a}", a if re.fullmatch(r"\w+", a) else f"({a})")
        kk, v = self.kind(t)
        if kk in ("record", "variant", "enum"):
            return f"decode{self.named(t)}({a})"
        if kk == "option":
            po = self.variant_layout([None, v])[2]
            return f"(if (loadByte({a}).toInt() == 0) null else {self.lift(v, f'({a}) + {po}', depth + 1)})"
        if kk == "list":
            p, n, i = f"p{depth}", f"n{depth}", f"i{depth}"
            es, ea = self.layout(v)
            stride = align_to(es, ea)
            if self.resolve(v) == "u8":
                return f"run {{ val {p} = loadInt({a}); ByteArray(loadInt(({a}) + 4)) {{ loadByte({p} + it) }} }}"
            elem = self.lift(v, f"{p} + {i} * {stride}", depth + 1)
            return f"run {{ val {p} = loadInt({a}); List(loadInt(({a}) + 4)) {{ {i} -> {elem} }} }}"
        if kk == "tuple":
            _, _, offs = self.record_layout(v["types"])
            parts = [self.lift(x, f"({a}) + {o}", depth + 1) for x, o in zip(v["types"], offs)]
            return f"{'Pair' if len(parts) == 2 else 'Triple'}({', '.join(parts)})"
        if kk == "result":
            po = self.variant_layout([v.get("ok"), v.get("err")])[2]
            ok = "Unit" if v.get("ok") is None else self.lift(v["ok"], f"({a}) + {po}", depth + 1)
            err = "Unit" if v.get("err") is None else self.lift(v["err"], f"({a}) + {po}", depth + 1)
            return f"(if (loadByte({a}).toInt() == 0) Either.Right({ok}) else Either.Left({err}))"
        die(f"cannot lift {kk}")

    # ---- named types --------------------------------------------------------------------------
    def named(self, t):
        d = self.tdef(t)
        name = pascal(d["name"])
        if name in self.decls or name in self.lifts:
            return name
        self.lifts[name] = ""  # reserve (guards against re-entry)
        kk, v = self.kind(t)
        doc = self.doc(d)
        if kk == "record":
            fields = v["fields"]
            _, _, offs = self.record_layout([f["type"] for f in fields])
            props = ", ".join(f"val {camel(f['name'])}: {self.ktype(f['type'])}" for f in fields)
            if not fields:
                die(f"empty record {d['name']}")
            args = ", ".join(self.lift(f["type"], f"a + {o}") for f, o in zip(fields, offs))
            self.decls[name] = f"{doc}data class {name}({props})"
            self.lifts[name] = f"private fun decode{name}(a: Int): {name} = {name}({args})"
        elif kk == "enum":
            cases = [screaming(c["name"]) for c in v["cases"]]
            self.decls[name] = f"{doc}enum class {name} {{ {', '.join(cases)} }}"
            self.lifts[name] = f"private fun decode{name}(a: Int): {name} = {name}.entries[{self.lift_tag(len(cases))}]"
        else:
            self.variant(name, d, v, doc)
        self.order.append(name)
        return name

    def lift_tag(self, n):
        sz = self.tag_size(n)
        return {1: "(loadByte(a).toInt() and 0xFF)", 2: "(loadShort(a).toInt() and 0xFFFF)", 4: "loadInt(a)"}[sz]

    def variant(self, name, d, v, doc):
        cases = v["cases"]
        _, _, po = self.variant_layout([c.get("type") for c in cases])
        nested, branches = [], []
        for idx, c in enumerate(cases):
            cname = pascal(c["name"])
            cdoc = self.doc(c, indent="    ")
            pt = c.get("type")
            if pt is None:
                nested.append(f"{cdoc}object {cname} : {name}()")
                branches.append(f"        {idx} -> {name}.{cname}")
                continue
            rt = self.resolve(pt)
            flat = (not isinstance(rt, str) and not self.override_key(rt) and self.kind(rt)[0] == "record")
            if flat:
                fields = self.kind(rt)[1]["fields"]
                _, _, offs = self.record_layout([f["type"] for f in fields])
                props = ", ".join(f"val {camel(f['name'])}: {self.ktype(f['type'])}" for f in fields)
                args = ", ".join(self.lift(f["type"], f"a + {po + o}") for f, o in zip(fields, offs))
            else:
                props = f"val value: {self.ktype(pt)}"
                args = self.lift(pt, f"a + {po}")
            nested.append(f"{cdoc}data class {cname}({props}) : {name}()")
            branches.append(f"        {idx} -> {name}.{cname}({args})")
        body = "\n".join(nested)
        self.decls[name] = f"{doc}sealed class {name} {{\n{body}\n}}"
        br = "\n".join(branches)
        self.lifts[name] = (
            f"private fun decode{name}(a: Int): {name} = when (val tag = {self.lift_tag(len(cases))}) {{\n{br}\n"
            f"        else -> error(\"native decode{name}: unknown variant tag $tag\")\n    }}"
        )

    @staticmethod
    def doc(d, indent=""):
        c = (d.get("docs") or {}).get("contents")
        if not c:
            return indent
        lines = [l.rstrip().replace("*/", "*&#47;") for l in c.strip().split("\n")]
        return indent + "/**\n" + "\n".join(f"{indent} * {l}".rstrip() for l in lines) + f"\n{indent} */\n" + indent

    # ---- driver -------------------------------------------------------------------------------
    def root(self):
        for i, d in enumerate(self.types):
            if d.get("name") == ROOT_NAME:
                o = d.get("owner") or {}
                if "interface" in o and self.ifaces[o["interface"]]["name"] == ROOT_INTERFACE:
                    return i
        die(f"{ROOT_NAME} not found")

    def read_results(self):
        """`get-next` of get-oplog / search-oplog: `result<option<list<..>>, oplog-read-error>`."""
        oplog = next(i for i in self.ifaces if i["name"] == ROOT_INTERFACE)
        out = []
        for fn, const, lifter in (("[method]get-oplog.get-next", "GET_OPLOG_NEXT", "liftGetOplogNext"),
                                  ("[method]search-oplog.get-next", "SEARCH_OPLOG_NEXT", "liftSearchOplogNext")):
            rt = oplog["functions"][fn]["result"]
            size, align = self.layout(rt)
            out += [f"/** Return-area size/alignment of `{fn}`. */",
                    f"internal const val {const}_SIZE = {size}",
                    f"internal const val {const}_ALIGN = {align}",
                    f"internal fun {lifter}(base: Int): {self.ktype(rt)} = {self.lift(rt, 'base')}", ""]
        return out

    def strip(self, t):
        """The underlying definition of `t`, following every alias (including overridden ones)."""
        kk, v = self.kind(t)
        while kk == "type":
            if isinstance(v, str):
                return v, None
            kk, v = self.kind(v)
        return kk, v

    def check_overrides(self):
        """Fails loudly if a hand-written layout assumption in OVERRIDES no longer matches the WIT."""
        for i, d in enumerate(self.types):
            if d.get("name") == "attribute-value" and self.override_key(i):
                kk, v = self.strip(i)
                if kk != "variant" or [c["name"] for c in v["cases"]] != ["string"] or self.variant_layout([c.get("type") for c in v["cases"]])[2] != 4:
                    die("attribute-value changed shape; update the OVERRIDES entries")
            if d.get("name") == "attribute" and self.override_key(i):
                kk, v = self.strip(i)
                offs = self.record_layout([f["type"] for f in v["fields"]])[2]
                if [f["name"] for f in v["fields"]] != ["key", "value"] or offs != [0, 8]:
                    die("attribute changed shape; update the OVERRIDES entries")

    def render(self):
        self.check_overrides()
        root = self.root()
        self.named(root)
        size, align = self.layout(root)
        reads = self.read_results()
        parts = [
            "// GENERATED by scripts/gen-oplog.py from wit-native/deps/golem-1.x/golem-oplog.wit -- do not edit.",
            "// Re-run the script after scripts/sync-wit.sh. Field offsets and variant tags come from the WIT.",
            "@file:Suppress(\"ktlint\") // generated: not hand-formatted (ktlint exclusions do not apply to the source-set tasks)",
            "@file:OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class, kotlin.wasm.ExperimentalWasmInterop::class)",
            "",
            "package cloud.golem.runtime.host",
            "",
            "@@IMPORTS@@",
            "",
            f"/** Size in bytes of one `{ROOT_NAME}` in linear memory. */",
            f"internal const val PUBLIC_OPLOG_ENTRY_SIZE = {size}",
            "",
            f"internal fun liftPublicOplogEntry(base: Int): PublicOplogEntry = decode{pascal(ROOT_NAME)}(base)",
            "",
        ]
        parts.extend(reads)
        for name in sorted(self.decls):
            parts.append(self.decls[name])
            parts.append("")
        for name in sorted(self.lifts):
            parts.append(self.lifts[name])
            parts.append("")
        body = "\n".join(parts)
        wanted = [
            "cloud.golem.runtime.AgentId", "cloud.golem.runtime.ComponentId", "cloud.golem.runtime.Either",
            "cloud.golem.runtime.EnvironmentId", "cloud.golem.runtime.TypedSchemaValue", "cloud.golem.runtime.Uuid",
            "cloud.golem.runtime.liftTypedSchemaValue", "cloud.golem.wasm.liftString", "cloud.golem.wasm.loadByte",
            "cloud.golem.wasm.loadDouble", "cloud.golem.wasm.loadFloat", "cloud.golem.wasm.loadInt",
            "cloud.golem.wasm.loadLong", "cloud.golem.wasm.loadShort",
        ]
        # Only import what the generated code uses (the formatter would delete unused imports and make --check fail).
        used = [i for i in wanted if re.search(r"\b" + re.escape(i.rsplit(".", 1)[1]) + r"\b", body.replace("@@IMPORTS@@", ""))]
        body = body.replace('@@IMPORTS@@', "\n".join("import " + i for i in sorted(used)))
        return body.rstrip() + "\n"


def main():
    text = Gen(load_json()).render()
    if "--wit-dir" in sys.argv:
        print("\n".join(l for l in text.splitlines() if "_SIZE" in l and "const" in l))
        return
    if "--check" in sys.argv:
        if not OUT.exists() or OUT.read_text() != text:
            die(f"{OUT} is out of date -- run scripts/gen-oplog.py")
        return
    OUT.write_text(text)
    print(f"wrote {OUT} ({len(text.splitlines())} lines)")


if __name__ == "__main__":
    main()
