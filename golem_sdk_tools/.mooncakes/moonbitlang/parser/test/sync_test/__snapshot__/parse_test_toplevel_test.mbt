test "nothing" {

}

fn assirt(x : Bool) {
	if x {
		println("succeed")
	} else {
		println("failed")
	}
}

test "property.name.test" {
  assirt(1+1 == 2)
}

test "property2.name.test" {
  let test1 = 1 - 1
  assirt(test1 == 0)
}

test {
	assirt(1+1 == 2)
}
