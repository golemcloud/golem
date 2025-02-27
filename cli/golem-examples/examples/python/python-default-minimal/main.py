from component_name import exports

state: int = 0

class Api(exports.Api):
    def add(self, value: int):
      global state
      print("add " + str(value))
      state = state + value 

    def get(self) -> int:
       global state
       print("get")
       return state
