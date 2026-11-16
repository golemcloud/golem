// Package staticdefaults declares the shared static-config reference agent.
package staticdefaults

import "github.com/golemcloud/golem/sdks/go/golem"

type ID struct{ Name string }

type Settings struct {
	Label string
	Count uint32
}

type Choice interface{ choice() }
type Text struct{ Value string }
type Other struct{ Value string }

func (Text) choice()  {}
func (Other) choice() {}

var _ = golem.DefineUnion[Choice](
	golem.WrappedBranch[Text]("Text", golem.ByPrefix("selected:")),
	golem.WrappedBranch[Other]("Other", golem.ByPrefix("other:")),
)

type Config struct {
	Settings Settings                             `golemDefault:"{\"label\":\"code-default\",\"count\":31}"`
	Choice   Choice                               `golemDefault:"\"selected:value\""`
	Slots    golem.Tuple3[*int32, *int32, string] `golemDefault:"[null,7,\"retained\"]"`
	Omitted  *string
}

var Agent = golem.DefineConfiguredAgent[ID, Config](golem.Spec{
	Name: "StaticDefaultsAgent", Mode: golem.Durable,
})

var (
	Read            = Agent.Method[golem.Unit, string]("read")
	ConstructorRead = Agent.Method[golem.Unit, string]("constructor-read")
)
