async fn[T] callcc(cb : ((T) -> Unit) -> Unit) -> T noraise = "%async.suspend"
fn run_async(f : async () -> Unit noraise) -> Unit = "%async.run"

struct Scheduler {
  mut next_id : Int
  coros : @queue.Queue[() -> Unit]
}

fn Scheduler::new() -> Scheduler {
  { next_id: 0, coros: @queue.new() }
}

fn Scheduler::step(self : Scheduler) -> Unit {
  match self.coros.pop() {
    None => ()
    Some(coro) => coro()
  }
}

async fn Scheduler::suspend(self : Scheduler) -> Unit noraise {
  callcc(fn (k) { self.coros.push(fn () { k(()) }) })
}

fn Scheduler::run_all(self : Scheduler) -> Unit {
  loop self.coros.pop() {
    None => ()
    Some(coro) => {
      coro()
      continue self.coros.pop()
    }
  }
}

fn Scheduler::spawn_lazy(self : Scheduler, f : async (Int) -> Unit noraise) -> Unit {
  let id = self.next_id
  self.next_id += 1
  self.coros.push(fn () {
    run_async(fn () { f(id) })
  })
}

fn Scheduler::spawn(self : Scheduler, f : async (Int) -> Unit noraise) -> Unit {
  let id = self.next_id
  self.next_id += 1
  run_async(fn () { f(id) })
}

fn init {
  let ctx = Scheduler::new()
  ctx.spawn_lazy(fn (id) {
    for i = 0; i < 10; i = i + 1 {
      println("thread \{id}: \{i}")
      ctx.suspend()
    }
    println("thread \{id}: done")
  })
  ctx.spawn(fn (id) {
    for i = 0; i < 5; i = i + 1 {
      println("thread \{id}: \{i}")
      ctx.suspend()
    }
    println("thread \{id}: done")
  })
  ctx.run_all()
}


// one-to-one, blocking channel
enum ChannelState[T] {
  No_value
  Has_value(T)
  Terminated
}

struct Channel[T] {
  ctx : Scheduler
  mut value : ChannelState[T]
}

fn[T] Scheduler::make_channel(self : Scheduler) -> Channel[T] {
  { ctx: self, value: No_value }
}

suberror ChannelTerminated

fn[T] Channel::terminate(self : Channel[T]) -> Unit {
  self.value = Terminated
}

async fn[T] Channel::send(self : Channel[T], value : T) -> Unit raise ChannelTerminated {
  match self.value {
    Terminated => raise ChannelTerminated
    Has_value(_) => abort("multiple sender detected")
    No_value => self.value = Has_value(value)
  }
  // wait until the value is read
  loop self.value {
    Has_value(_) => {
      self.ctx.suspend()
      continue self.value
    }
    No_value => ()
    Terminated => raise ChannelTerminated
  }
}

async fn[T] Channel::recv(self : Channel[T]) -> T raise ChannelTerminated {
  loop self.value {
    Has_value(value) => {
      self.value = No_value
      value
    }
    No_value => {
      self.ctx.suspend()
      continue self.value
    }
    Terminated => raise ChannelTerminated
  }
}

fn init {
  let ctx = Scheduler::new()
  let c1 : Channel[Int] = ctx.make_channel()
  let c2 : Channel[Int] = ctx.make_channel()
  ctx.spawn(fn (id) {
    try {
      for {
        println("thread \{id}: received \{c1.recv()} from channel 1")
      }
    } catch {
      ChannelTerminated => println("thread \{id}: received termination")
    }
  })
  ctx.spawn(fn (id) {
    try {
      for {
        println("thread \{id}: received \{c2.recv()} from channel 2")
      }
    } catch {
      ChannelTerminated => println("thread \{id}: received termination")
    }
  })
  ctx.spawn(fn (id) {
    try {
      println("thread \{id}: sending 1 to channel 1")
      c1.send(1)
      println("thread \{id}: sending 2 to channel 2")
      c2.send(2)
      println("thread \{id}: sending 3 to channel 2")
      c2.send(3)
      println("thread \{id}: sending 4 to channel 2")
      c1.send(4)
      println("thread \{id}: terminating channel 1")
      c1.terminate()
      println("thread \{id}: terminating channel 2")
      c2.terminate()
    } catch {
      ChannelTerminated => abort("unexpected termination of channel")
    }
  })
  ctx.run_all()
}
