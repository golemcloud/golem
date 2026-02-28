
struct Reference {
  mut contents:Int
}

fn init {
  let x = { contents:10, }
  letrec f_1 = () => {
    if x.contents == 0 {
      ()
    } else {
      println(x.contents)
      x.contents = x.contents - 1
      f_2()
    }
  }

  and f_2 = () => f_3()


  and f_3 = () => f_4()


  and f_4 = () => f_5()

  and f_5 = () => f_6()

  and f_6 = () => f_7()

  and f_7 = () => f_8()

  and f_8 = () => f_9()

  and f_9 = () => f_10()

  and f_10 = () => f_11()

  and f_11 = () => f_12()

  and f_12 = () => f_13()

  and f_13 = () => f_14()

  and f_14 = () => f_15()

  and f_15 = () => f_16()

  and f_16 = () => f_17()

  and f_17 = () => f_18()

  and f_18 = () => f_19()

  and f_19 = () => f_20()

  and f_20 = () => f_1()

  f_1()
}

