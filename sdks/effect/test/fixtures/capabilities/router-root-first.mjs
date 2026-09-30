import { HttpRouter as RootHttpRouter } from "@golemcloud/effect-golem"
import * as HttpRouter from "@golemcloud/effect-golem/HttpRouter"

if (RootHttpRouter.define !== HttpRouter.define) throw new Error("duplicate router registry")
