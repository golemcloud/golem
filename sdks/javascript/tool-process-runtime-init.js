// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1 (http://license.golem.cloud/LICENSE).

import process from "node:process";
import { setProcessBackend } from "wasm-rquickjs:child-process";
import * as native from "golem-tool-process-bridge";
import { createToolProcessBackend } from "golem-tool-process-backend";
import { createToolProcessHost } from "golem-tool-process-host";

// Registration runs during Wizer and in every fresh runtime. Tool discovery and
// authority resolution stay inside the backend's invocation methods.
setProcessBackend(
  createToolProcessBackend(createToolProcessHost(native), process),
);
