trait ObjectImpl {
  to_string(Self) -> String
  copy(Self) -> Object
  call_method(Self, Object, String) -> Object
  set_method(Self, String, (Object) -> Object) -> Unit
}


struct Object(&ObjectImpl)

fn[X: ObjectImpl] Object::make(x: X) -> Object {
  Object(x as &ObjectImpl)
}

fn Object::copy(self: Object) -> Object {
  self.0.copy()
}

impl Show for Object with output(self, logger) {
  logger.write_string(self.0.to_string())
}

fn Object::to_string(self : Object) -> String { Show::to_string(self) }

fn Object::call_method(self : Object, meth : String) -> Object {
  self.0.call_method(self, meth)
}

fn Object::set_method(self : Object, method_name : String, meth : (Object) -> Object) -> Unit {
  self.0.set_method(method_name, meth)
}


impl ObjectImpl for String with copy(self) {
  Object::make(self)
}

impl ObjectImpl for String with to_string(self) {
  self
}

impl ObjectImpl for String with call_method(_s, _self, meth) {
  abort("cannot call method \{meth} on string object")
}

impl ObjectImpl for String with set_method(_s, meth, _f) {
  abort("cannot set method \{meth} of string object")
}


struct FnObj {
  mut dispatch: (String) -> Option[(Object) -> Object]
}

fn FnObj::empty() -> Object {
  Object::make({ dispatch: _x => None })
}

impl ObjectImpl for FnObj with copy(self: FnObj) -> Object {
  Object::make({ dispatch: self.dispatch })
}

impl ObjectImpl for FnObj with to_string(_self) { "<fn-object>" }

impl ObjectImpl for FnObj with call_method(x, self, meth) {
  match (x.dispatch)(meth) {
    None => abort("no such method \{meth}")
    Some(f) => f(self)
  }
}

impl ObjectImpl for FnObj with set_method(
  self: FnObj,
  new_method_name: String,
  meth: (Object) -> Object
) -> Unit {
  let old_dispatch = self.dispatch
  self.dispatch = fn (method_name) {
    if new_method_name == method_name {
      Some(meth)
    } else {
      old_dispatch(method_name)
    }
  }
}


let duck: Object = FnObj::empty()
fn init {
  duck.set_method("speak", _x => Object::make("Quak!"))
}

fn speak_loud(base: Object) -> Object {
  let obj = base.copy()
  obj.set_method("speak_loud", fn (self) {
    let message = self.call_method("speak").to_string() + "!!!"
    Object::make(message)
  })
  obj
}

fn speak_name(base: Object) -> Object {
  let obj = base.copy()
  obj.set_method("speak", fn (self) {
    let name = self.call_method("name").to_string()
    let message = base.0.call_method(self, "speak").to_string()
    Object::make("\{name}: \{message}")
  })
  obj
}

fn init {
  let duck1 = speak_name(duck)
  duck1.set_method("name", fn (_self) { Object::make("duck1") })
  println(duck1.call_method("speak"))

  let duck2 = speak_name(duck)
  duck2.set_method("name", fn (_self) { Object::make("duck2") })
  println(duck2.call_method("speak"))

  let anonymous_loud_duck = speak_loud(duck)
  println(anonymous_loud_duck.call_method("speak"))
  println(anonymous_loud_duck.call_method("speak_loud"))

  let duck3 = speak_name(anonymous_loud_duck)
  duck3.set_method("name", fn (_self) { Object::make("duck3") })
  println(duck3.call_method("speak"))
  println(duck3.call_method("speak_loud"))
}
