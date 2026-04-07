export interface FailureClassification {
  code: string;
  category:
    | "agent"
    | "build"
    | "deploy"
    | "assertion"
    | "network"
    | "infra"
    | "unknown";
  guidance: string;
}

const CLASSIFICATION_MAP: Record<
  string,
  { category: FailureClassification["category"]; guidance: string }
> = {
  SKILL_NOT_ACTIVATED: {
    category: "agent",
    guidance:
      "Ensure the agent reads SKILL.md. Check the watcher directory and verify skills are copied correctly.",
  },
  SKILL_MISMATCH: {
    category: "agent",
    guidance:
      "Unexpected skills were activated. Review allowedExtraSkills or enable strictSkillMatch.",
  },
  BUILD_FAILED: {
    category: "build",
    guidance:
      "Check that golem.yaml exists and the component builds locally. Review build output for compilation errors.",
  },
  DEPLOY_FAILED: {
    category: "deploy",
    guidance:
      "Verify the Golem server is running and the component was built successfully before deploying.",
  },
  INVOKE_FAILED: {
    category: "deploy",
    guidance:
      "Function invocation failed. Verify the component is deployed and the function name matches the interface.",
  },
  INVOKE_JSON_FAILED: {
    category: "deploy",
    guidance:
      "JSON function invocation failed. Verify the component is deployed and the function name matches the interface.",
  },
  SHELL_FAILED: {
    category: "infra",
    guidance:
      "Shell command returned non-zero exit code. Check the command, arguments, and working directory.",
  },
  HTTP_FAILED: {
    category: "network",
    guidance:
      "HTTP request failed. Check the URL, ensure the server is available, and verify expected response status.",
  },
  CREATE_AGENT_FAILED: {
    category: "infra",
    guidance:
      "Failed to create a new agent. Verify the Golem CLI is installed and the server is reachable.",
  },
  DELETE_AGENT_FAILED: {
    category: "infra",
    guidance:
      "Failed to delete an agent. The agent may not exist or the Golem server may be unreachable.",
  },
  ASSERTION_FAILED: {
    category: "assertion",
    guidance:
      "Output did not match expected assertions. Review the expect block and compare with actual output.",
  },
};

export function classifyFailure(errorString: string): FailureClassification {
  for (const [prefix, classification] of Object.entries(CLASSIFICATION_MAP)) {
    if (errorString.startsWith(prefix)) {
      return {
        code: prefix,
        ...classification,
      };
    }
  }

  // Check for agent-related errors that don't have a prefix
  if (errorString.startsWith("Agent failed:")) {
    return {
      code: "AGENT_FAILED",
      category: "agent",
      guidance:
        "The agent returned an error. Check agent logs and ensure the prompt is clear.",
    };
  }

  return {
    code: "UNKNOWN",
    category: "unknown",
    guidance:
      "An unclassified error occurred. Review the full error output for details.",
  };
}
