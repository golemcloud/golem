state: int = 0

from binding.pack_name import exports
# Example common lib import
# from lib import example_common_function

class ComponentNameApi(exports.ComponentNameApi):
    def add(self, value: int):
        global state
        print("add " + str(value))
        state = state + value

    def get(self) -> int:
        global state
        print("get")
        return state
