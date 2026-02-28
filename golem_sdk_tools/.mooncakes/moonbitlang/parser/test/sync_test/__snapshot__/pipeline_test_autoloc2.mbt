//!build: -w -27

fn hello(loc~:SourceLoc=_) -> Unit {
  println("caller\{loc}")  
}

fn hello2(loc~:SourceLoc=_) -> Unit {
  println("caller\{loc}")  
  hello(loc~)
}

fn init {
  hello2()
}

