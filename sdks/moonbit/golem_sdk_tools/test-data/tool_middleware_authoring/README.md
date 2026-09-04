# Tool-middleware authoring contract fixtures

These files freeze the GOL-34 annotation names, metadata/defaults, placement, role scenarios, exact
handler signatures, and behavioral intent. They are non-generated parser inputs. The corresponding
compile fixture is `golem_sdk/tool-middleware-authoring-test`; it exercises the locked public SDK
types and every monomorphic result projection.

The fixtures intentionally cover transparent, adapter, short-circuit, retry, universal, and
combined component forms. Presented and expected monomorphic tool declarations are in the same
source package. Cross-package tool-shape references are outside the initial GOL-34 contract.
