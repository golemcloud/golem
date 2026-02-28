//!build: -w -67

async fn[T, E : Error] suspend(f : ((T) -> Unit, (E) -> Unit) -> Unit) -> T raise E = "%async.suspend"
fn run_async(f : async () -> Unit noraise) -> Unit = "%async.run"

suberror Err derive(Show)

fn sync_with_err() -> Unit raise Err {
  raise Err
}

async fn async_to_result() -> Unit noraise {
  println(try? sync_with_err())
}

fn init {
  run_async(async_to_result)
}

async fn async_with_err() -> Unit raise {
  sync_with_err()
}

fn init {
  run_async(async fn () -> Unit noraise {
    try {
      async_with_err()
    } catch {
      err => println("catched: \{err}")
    }
  })
}

async fn async_with_err_non_tail() -> Unit raise {
  sync_with_err()
  println("!!!")
}

fn init {
  run_async(fn () {
    try {
      async_with_err_non_tail()
    } catch {
      err => println("catched: \{err}")
    }
  })
}

async fn async_with_try() -> Unit noraise {
  try {
    sync_with_err()
  } catch {
    err => println("catched: \{err}")
  }
}

fn init {
  run_async(async_with_try)
}

async fn async_with_tail_err_in_if(c : Bool) -> Unit raise Err {
  if c {
    sync_with_err()
  }
}

fn init {
  run_async(fn () {
    try {
      async_with_tail_err_in_if(true)
    } catch {
      err => println("catched: \{err}")
    }
  })
}

trait Obj_with_err {
  method_with_err(Self) -> Unit raise Err
}

impl Obj_with_err for Int with method_with_err(_self) {
  raise Err
}

async fn async_with_tail_err_obj_method(obj : &Obj_with_err) -> Unit raise Err {
  obj.method_with_err()
}

fn init {
  run_async(fn () {
    try {
      async_with_tail_err_obj_method(42)
    } catch {
      err => println("catched: \{err}")
    }
  })
}

async fn raise_in_sync_with_async_catch(input : String?) -> Unit {
  try {
    let result = match input {
      None => raise Err
      Some(input) => input
    }
    println(result)
  } catch {
    _ => async_with_err()
  }
}

fn init {
  run_async(fn() { try! raise_in_sync_with_async_catch(Some("msg")) })
}
