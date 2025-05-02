# make sure this stays before other urllib uses
from urllib3.contrib.wasi import enable_wasi_backend
enable_wasi_backend("component_name")

from pack_name import exports
# Example common lib import
from lib import example_common_function

state: int = 0

class ComponentNameApi(exports.ComponentNameApi):
    def add(self, value: int):
        global state
        print("add " + str(value))
        state = state + value

    def get(self) -> int:
        global state
        print("get")
        return state
