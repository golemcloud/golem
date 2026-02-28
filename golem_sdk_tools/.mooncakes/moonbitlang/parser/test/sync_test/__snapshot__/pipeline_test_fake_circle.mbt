struct Reference {
  mut contents:Int
}

fn init {
  let x = { contents:10, }
  letrec f_1 = fn() {
    if x.contents == 0 {
      ()
    } else {
      println(x.contents)
      x.contents = x.contents - 1
      f_2()
    }
  }

  and f_2 = fn () {
    f_3()
  }

  and f_3 = fn () {
    f_4()
  }

  and f_4 = fn () {
    f_5()
  }

  and f_5 = fn () {
    f_6()
  }

  and f_6 = fn () {
    f_7()
  }

  and f_7 = fn () {
    f_8()
  }

  and f_8 = fn () {
    f_9()
  }

  and f_9 = fn () {
    f_10()
  }

  and f_10 = fn () {
    f_11()
  }

  and f_11 = fn () {
    f_12()
  }

  and f_12 = fn () {
    f_13()
  }

  and f_13 = fn () {
    f_14()
  }

  and f_14 = fn () {
    f_15()
  }

  and f_15 = fn () {
    f_16()
  }

  and f_16 = fn () {
    f_17()
  }

  and f_17 = fn () {
    f_18()
  }

  and f_18 = fn () {
    f_19()
  }

  and f_19 = fn () {
    f_20()
  }

  and f_20 = fn () {
    ()
  }

  f_1()
}

