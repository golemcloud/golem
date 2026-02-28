fn init {
	a |> f 
}

fn init {
	a |> f(b,c) 
}

fn init {
	a |> f1(b,c) |> f2(d,e)
}

fn init {
	a |> f1 |>  //unfinished statement
	action2() //another statement
}

test {
	a |>
	next	
}

test {
	a |>

	next2	
}

test {
	a |> _
	b 	
}