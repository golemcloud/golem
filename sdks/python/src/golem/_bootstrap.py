"""The module that pre-initialization imports.

The assembler passes the application's entry modules, in order, in the
`GOLEM_PYTHON_ENTRY_MODULES` environment variable, separated by commas.  The
bootstrap registers the component's exports, imports each entry module,
finalizes the agent registry and removes the variable, so that the saved
component does not keep it.
"""

import importlib
import os

from golem import _guest, _registry  # noqa: F401  (registers the exports)

_ENTRY_MODULES = "GOLEM_PYTHON_ENTRY_MODULES"

for _module in os.environ.pop(_ENTRY_MODULES).split(","):
    importlib.import_module(_module)

_registry.finalize()
