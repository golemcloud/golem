import { Schema } from "effect"
import { defineConfig } from "@golemcloud/effect-golem"

export class AgentConfig extends defineConfig("RuntimeMetadata.Config", {
  apiUrl: Schema.String,
  apiKey: Schema.Redacted(Schema.String),
}) {}
