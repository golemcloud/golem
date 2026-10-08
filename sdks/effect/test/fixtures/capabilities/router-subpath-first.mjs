import * as HttpRouter from "@golemcloud/effect-golem/HttpRouter"
import { HttpRouter as RootHttpRouter } from "@golemcloud/effect-golem"

if (HttpRouter.define !== RootHttpRouter.define) throw new Error("duplicate router registry")
