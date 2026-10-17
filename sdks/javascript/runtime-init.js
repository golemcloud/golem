// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1 (http://license.golem.cloud/LICENSE).

import process from "node:process";
import { setProcessBackend } from "wasm-rquickjs:child-process";
import * as host from "golem:tool/host@0.1.0";
import { createToolProcessBackend } from "golem-tool-process-backend";

setProcessBackend(createToolProcessBackend(host, process));
