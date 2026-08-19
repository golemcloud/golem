package main

// The blank import initializes the Golem SDK runtime (for example, routing
// net/http through the platform); keep it even though it looks unused. Each
// agent package below registers itself on import, and main stays empty.
import (
	_ "github.com/golemcloud/golem/sdks/go/golem"
)

func main() {}
