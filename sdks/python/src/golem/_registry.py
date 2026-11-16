"""Agent types that the application's entry modules declare.

The bootstrap finalizes the registry once, after it imports every entry
module and before pre-initialization saves the component.  Discovery reads
the finalized registry; it constructs no agents.
"""

_finalized = False


def finalize() -> None:
    global _finalized
    if _finalized:
        raise RuntimeError("the agent registry is already finalized")
    _finalized = True


def agent_types() -> list:
    if not _finalized:
        raise RuntimeError("the agent registry is not finalized")
    return []
