module golem.com/tinygo_wasi

go 1.20

require github.com/golemcloud/golem-go v0.4.4

// TODO: update version and remove override before merge
replace (
	github.com/golemcloud/golem-go => ../../../golem-go
)