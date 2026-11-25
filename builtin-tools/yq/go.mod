module golem-yq

go 1.27.1

require (
	github.com/golemcloud/golem/sdks/go/golem v0.0.0
	github.com/mikefarah/yq/v4 v4.54.3
)

require (
	github.com/a8m/envsubst v1.4.3 // indirect
	github.com/agext/levenshtein v1.2.1 // indirect
	github.com/alecthomas/participle/v2 v2.1.4 // indirect
	github.com/apparentlymart/go-textseg/v15 v15.0.0 // indirect
	github.com/apparentlymart/go-textseg/v17 v17.0.1 // indirect
	github.com/apparentlymart/go-userdirs v0.0.0-20200915174352-b0c018a67c13 // indirect
	github.com/bytecodealliance/componentize-go v0.4.3 // indirect
	github.com/dimchansky/utfbom v1.2.0 // indirect
	github.com/elliotchance/orderedmap v1.8.0 // indirect
	github.com/fatih/color v1.19.0 // indirect
	github.com/go-ini/ini v1.67.0 // indirect
	github.com/goccy/go-json v0.12.0 // indirect
	github.com/goccy/go-yaml v1.19.2 // indirect
	github.com/gofrs/flock v0.13.0 // indirect
	github.com/golemcloud/golem/sdks/go/core v0.1.2 // indirect
	github.com/google/go-cmp v0.6.0 // indirect
	github.com/hashicorp/hcl/v2 v2.25.0 // indirect
	github.com/inconshreveable/mousetrap v1.1.0 // indirect
	github.com/jinzhu/copier v0.4.0 // indirect
	github.com/magiconair/properties v1.18.12 // indirect
	github.com/mattn/go-colorable v0.1.14 // indirect
	github.com/mattn/go-isatty v0.0.24 // indirect
	github.com/mitchellh/go-wordwrap v1.0.1 // indirect
	github.com/pelletier/go-toml/v2 v2.4.3 // indirect
	github.com/spf13/cobra v1.10.2 // indirect
	github.com/spf13/pflag v1.0.10 // indirect
	github.com/yuin/gopher-lua v1.1.2 // indirect
	github.com/zclconf/go-cty v1.19.0 // indirect
	go.bytecodealliance.org/pkg v0.2.3 // indirect
	go.yaml.in/yaml/v4 v4.0.0-rc.7 // indirect
	golang.org/x/mod v0.42.0 // indirect
	golang.org/x/net v0.62.0 // indirect
	golang.org/x/sync v0.24.0 // indirect
	golang.org/x/sys v0.49.0 // indirect
	golang.org/x/text v0.43.0 // indirect
	golang.org/x/tools v0.51.0 // indirect
)

tool github.com/bytecodealliance/componentize-go

// Golem's yq fork adds stdio hooks, a wasip1 read-only check and a hook for
// the system operator; see README.md.
replace github.com/mikefarah/yq/v4 => github.com/golemcloud/yq/v4 v4.54.3-golem.2

replace go.bytecodealliance.org/pkg => github.com/golemcloud/go-pkg v0.2.3-golem.2

replace github.com/golemcloud/golem/sdks/go/golem => ../../sdks/go/golem

replace github.com/golemcloud/golem/sdks/go/core => ../../sdks/go/core
