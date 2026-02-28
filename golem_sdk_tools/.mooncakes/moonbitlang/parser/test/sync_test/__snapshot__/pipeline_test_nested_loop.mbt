fn init {
  fn nested_loop(n : Int) {
    fn lp_i(i : Int) {
      if i < n {
        fn lp_j(j : Int) {
          if j < n {
            println(i + j)
            lp_j(j + 1)
          }
        }

        lp_j(0)
        lp_i(i + 1)
      }
    }

    lp_i(0)
  }

  nested_loop(10)
}

