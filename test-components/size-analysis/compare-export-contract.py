#!/usr/bin/env python3
"""Compare `wasm-tools component wit --json` outputs across SDK optimizations.

Ignore declaration order and dead imports, but preserve the five guest
interfaces, their named types, and all function signatures and resource owners.
No toolchain processes are launched by this check.
"""
import functools
import hashlib
import json
import sys

EXPECTED = {
    "golem:agent/guest@2.0.0",
    "golem:tool/guest@0.1.0",
    "golem:tool/tool-middleware-guest@0.1.0",
    "golem:api/load-snapshot@1.5.0",
    "golem:api/save-snapshot@1.5.0",
}


def contract(path):
    with open(path) as file:
        wit = json.load(file)

    def interface_name(index):
        interface = wit["interfaces"][index]
        package = wit["packages"][interface["package"]]["name"]
        name, separator, version = package.partition("@")
        return f'{name}/{interface["name"]}{separator}{version}'

    @functools.cache
    def type_hash(index):
        ty = wit["types"][index]
        if ty["kind"] == "resource":
            return f'{interface_name(ty["owner"]["interface"])}/{ty["name"]}'
        kind = ty["kind"]
        # An alias does not change the canonical type's identity.
        if isinstance(kind, dict) and set(kind) == {"type"}:
            return normalize(kind["type"])
        encoded = json.dumps(normalize(kind), sort_keys=True).encode()
        return hashlib.sha256(encoded).hexdigest()

    def normalize(value):
        if isinstance(value, int):
            return type_hash(value)
        if isinstance(value, list):
            return [normalize(item) for item in value]
        if isinstance(value, dict):
            return {key: normalize(item) for key, item in value.items()
                    if key not in {"docs", "stability"}}
        return value

    def interfaces(items):
        result = {}
        for item in items.values():
            index = item["interface"]["id"]
            interface = wit["interfaces"][index]
            result[interface_name(index)] = {
                "types": {name: type_hash(ty) for name, ty in interface["types"].items()},
                "functions": normalize(interface["functions"]),
            }
        return result

    assert len(wit["worlds"]) == 1
    world = wit["worlds"][0]
    return interfaces(world["exports"]), interfaces(world["imports"])


if __name__ == "__main__":
    before_exports, before_imports = contract(sys.argv[1])
    after_exports, after_imports = contract(sys.argv[2])
    assert set(before_exports) == set(after_exports) == EXPECTED
    for name in EXPECTED:
        assert before_exports[name] == after_exports[name], f"changed export: {name}"
    assert after_imports.keys() <= before_imports.keys(), "new import interface"
    for name, interface in after_imports.items():
        for category in ("types", "functions"):
            for member, signature in interface[category].items():
                assert before_imports[name][category].get(member) == signature, (
                    f"new or changed import: {name}#{member}"
                )
    print("five-interface export contract unchanged; no new imports")
