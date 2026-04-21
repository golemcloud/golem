import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { Socket } from "node:net";
import * as path from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { fileURLToPath } from "node:url";
import * as log from "./log.js";

export type PrerequisiteServiceName = "postgres" | "mysql" | "ignite" | "openai-mock";

interface ManagedService {
  readonly name: PrerequisiteServiceName;
  readonly containerId: string;
  readonly env: Record<string, string>;
  readonly variables: Record<string, string>;
}

export interface StartedPrerequisiteServices {
  readonly env: Record<string, string>;
  readonly variables: Record<string, string>;
  stopAll(): Promise<void>;
}

const POSTGRES_IMAGE = "postgres:16";
const MYSQL_IMAGE = "mysql:8";
const IGNITE_IMAGE = "apacheignite/ignite:2.17.0";
const WIREMOCK_IMAGE = "wiremock/wiremock:3.13.0";

export async function startPrerequisiteServices(
  names: PrerequisiteServiceName[] | undefined,
): Promise<StartedPrerequisiteServices> {
  const uniqueNames = Array.from(new Set(names ?? []));
  if (uniqueNames.length === 0) {
    return emptyServices();
  }

  const started: ManagedService[] = [];
  try {
    for (const name of uniqueNames) {
      started.push(await startService(name));
    }
  } catch (error) {
    await stopManagedServices(started);
    throw error;
  }

  const env: Record<string, string> = {};
  const variables: Record<string, string> = {};
  for (const service of started) {
    Object.assign(env, service.env);
    Object.assign(variables, service.variables);
  }

  return {
    env,
    variables,
    stopAll: async () => {
      await stopManagedServices(started);
    },
  };
}

function emptyServices(): StartedPrerequisiteServices {
  return {
    env: {},
    variables: {},
    stopAll: async () => {
      // no-op
    },
  };
}

async function startService(name: PrerequisiteServiceName): Promise<ManagedService> {
  switch (name) {
    case "postgres":
      return startPostgresService();
    case "mysql":
      return startMySqlService();
    case "ignite":
      return startIgniteService();
    case "openai-mock":
      return startOpenAiMockService();
  }
}

async function startPostgresService(): Promise<ManagedService> {
  log.info(`Starting prerequisite service postgres with ${POSTGRES_IMAGE}`);
  const containerId = await dockerRunDetached([
    "run",
    "-d",
    "--rm",
    "-p",
    "127.0.0.1::5432",
    "-e",
    "POSTGRES_USER=postgres",
    "-e",
    "POSTGRES_PASSWORD=postgres",
    "-e",
    "POSTGRES_DB=golem_test",
    "--health-cmd=pg_isready -U postgres -d golem_test",
    "--health-interval=5s",
    "--health-timeout=5s",
    "--health-retries=20",
    POSTGRES_IMAGE,
  ]);

  try {
    const hostPort = await getMappedPort(containerId, 5432);
    await waitForContainerHealth(containerId, "healthy", 60_000);
    await waitForTcpReady("127.0.0.1", hostPort, 30_000);

    const url = `postgres://postgres:postgres@127.0.0.1:${hostPort}/golem_test`;
    return {
      name: "postgres",
      containerId,
      env: {
        DATABASE_URL: url,
        POSTGRES_URL: url,
        DB_POSTGRES_URL: url,
      },
      variables: {
        postgres_url: url,
      },
    };
  } catch (error) {
    await stopManagedServices([{ name: "postgres", containerId }]);
    throw error;
  }
}

async function startMySqlService(): Promise<ManagedService> {
  log.info(`Starting prerequisite service mysql with ${MYSQL_IMAGE}`);
  const containerId = await dockerRunDetached([
    "run",
    "-d",
    "--rm",
    "-p",
    "127.0.0.1::3306",
    "-e",
    "MYSQL_ROOT_PASSWORD=golem",
    "-e",
    "MYSQL_DATABASE=golem_test",
    "--health-cmd=mysqladmin ping -h 127.0.0.1 -pgolem --silent",
    "--health-interval=5s",
    "--health-timeout=5s",
    "--health-retries=20",
    MYSQL_IMAGE,
  ]);

  try {
    const hostPort = await getMappedPort(containerId, 3306);
    await waitForContainerHealth(containerId, "healthy", 90_000);
    await waitForTcpReady("127.0.0.1", hostPort, 30_000);

    const url = `mysql://root:golem@127.0.0.1:${hostPort}/golem_test`;
    return {
      name: "mysql",
      containerId,
      env: {
        MYSQL_URL: url,
        DB_MYSQL_URL: url,
      },
      variables: {
        mysql_url: url,
      },
    };
  } catch (error) {
    await stopManagedServices([{ name: "mysql", containerId }]);
    throw error;
  }
}

async function startIgniteService(): Promise<ManagedService> {
  log.info(`Starting prerequisite service ignite with ${IGNITE_IMAGE}`);
  const containerId = await dockerRunDetached([
    "run",
    "-d",
    "--rm",
    "-p",
    "127.0.0.1::10800",
    "-e",
    "JVM_OPTS=-DIGNITE_ALLOW_DML_INSIDE_TRANSACTION=true",
    IGNITE_IMAGE,
  ]);

  try {
    const hostPort = await getMappedPort(containerId, 10800);
    await waitForLogMessage(containerId, "Ignite node started OK", 60_000);
    await waitForTcpReady("127.0.0.1", hostPort, 30_000);

    const url = `ignite://127.0.0.1:${hostPort}`;
    return {
      name: "ignite",
      containerId,
      env: {
        IGNITE_URL: url,
        DB_IGNITE_URL: url,
      },
      variables: {
        ignite_url: url,
      },
    };
  } catch (error) {
    await stopManagedServices([{ name: "ignite", containerId }]);
    throw error;
  }
}

async function startOpenAiMockService(): Promise<ManagedService> {
  log.info(`Starting prerequisite service openai-mock with ${WIREMOCK_IMAGE}`);

  // Resolve the harness root directory. When running via `npx tsx src/run.ts`, import.meta.url
  // points to `src/services.ts` (one level deep). When running compiled JS from `dist/src/`,
  // it is two levels deep. We walk up from the current file until we find `package.json`.
  const thisDir = path.dirname(fileURLToPath(import.meta.url));
  let harnessRoot = thisDir;
  while (!existsSync(path.join(harnessRoot, "package.json"))) {
    const parent = path.dirname(harnessRoot);
    if (parent === harnessRoot) break; // safety: filesystem root
    harnessRoot = parent;
  }
  const mappingsDir = path.resolve(harnessRoot, "docker", "openai-mock", "mappings");

  const containerId = await dockerRunDetached([
    "run",
    "-d",
    "--rm",
    "-p",
    "127.0.0.1::8080",
    "-v",
    `${mappingsDir}:/home/wiremock/mappings:ro`,
    WIREMOCK_IMAGE,
  ]);

  try {
    const hostPort = await getMappedPort(containerId, 8080);
    await waitForTcpReady("127.0.0.1", hostPort, 30_000);

    const url = `http://127.0.0.1:${hostPort}`;
    return {
      name: "openai-mock",
      containerId,
      env: {
        OPENAI_API_KEY: "test-mock-key",
        OPENAI_BASE_URL: `${url}/v1`,
      },
      variables: {
        openai_mock_url: url,
      },
    };
  } catch (error) {
    await stopManagedServices([{ name: "openai-mock", containerId }]);
    throw error;
  }
}

async function stopManagedServices(services: Pick<ManagedService, "name" | "containerId">[]): Promise<void> {
  for (const service of [...services].reverse()) {
    try {
      log.info(`Stopping prerequisite service ${service.name}`);
      await dockerRun(["rm", "-f", service.containerId]);
    } catch {
      // Ignore cleanup failures because containers may already be gone.
    }
  }
}

async function dockerRunDetached(args: string[]): Promise<string> {
  const result = await dockerRun(args);
  const containerId = result.stdout.trim();
  if (!containerId) {
    throw new Error(`docker ${args.join(" ")} did not return a container id`);
  }
  return containerId;
}

async function dockerRun(args: string[]): Promise<{ stdout: string; stderr: string }> {
  return new Promise((resolve, reject) => {
    const child = spawn("docker", args, {
      stdio: ["ignore", "pipe", "pipe"],
    });

    let stdout = "";
    let stderr = "";
    child.stdout?.on("data", (chunk) => {
      stdout += chunk.toString();
    });
    child.stderr?.on("data", (chunk) => {
      stderr += chunk.toString();
    });

    child.on("error", (error) => {
      reject(new Error(`Failed to run docker ${args.join(" ")}: ${error.message}`));
    });

    child.on("close", (code) => {
      if (code === 0) {
        resolve({ stdout, stderr });
        return;
      }

      reject(
        new Error(
          `docker ${args.join(" ")} failed with exit code ${code}: ${(stderr || stdout).trim()}`,
        ),
      );
    });
  });
}

async function getMappedPort(containerId: string, containerPort: number): Promise<number> {
  const result = await dockerRun(["port", containerId, `${containerPort}/tcp`]);
  const portSpec = result.stdout
    .trim()
    .split(/\r?\n/)
    .map((line) => line.trim())
    .find((line) => line.length > 0);

  if (!portSpec) {
    throw new Error(`docker port ${containerId} ${containerPort}/tcp returned no mapping`);
  }

  const portText = portSpec.slice(portSpec.lastIndexOf(":") + 1);
  const hostPort = Number(portText);
  if (!Number.isInteger(hostPort) || hostPort <= 0) {
    throw new Error(`Unable to parse mapped port from docker output: ${portSpec}`);
  }
  return hostPort;
}

async function waitForContainerHealth(
  containerId: string,
  expectedState: string,
  timeoutMs: number,
): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const result = await dockerRun([
      "inspect",
      "--format",
      "{{if .State.Health}}{{.State.Health.Status}}{{else}}{{.State.Status}}{{end}}",
      containerId,
    ]);
    const status = result.stdout.trim();
    if (status === expectedState) {
      return;
    }
    if (status === "unhealthy" || status === "exited" || status === "dead") {
      const logs = await safeDockerLogs(containerId);
      throw new Error(`Container ${containerId} became ${status}: ${logs}`);
    }
    await delay(1000);
  }

  const logs = await safeDockerLogs(containerId);
  throw new Error(`Timed out waiting for container ${containerId} to become ${expectedState}: ${logs}`);
}

async function waitForLogMessage(
  containerId: string,
  message: string,
  timeoutMs: number,
): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const logs = await safeDockerLogs(containerId);
    if (logs.includes(message)) {
      return;
    }
    await delay(1000);
  }

  const logs = await safeDockerLogs(containerId);
  throw new Error(`Timed out waiting for log message "${message}" from ${containerId}: ${logs}`);
}

async function safeDockerLogs(containerId: string): Promise<string> {
  try {
    const logs = await dockerRun(["logs", containerId]);
    return (logs.stdout + logs.stderr).trim();
  } catch (error) {
    return error instanceof Error ? error.message : String(error);
  }
}

async function waitForTcpReady(host: string, port: number, timeoutMs: number): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (await canConnect(host, port)) {
      return;
    }
    await delay(1000);
  }

  throw new Error(`Timed out waiting for TCP service on ${host}:${port}`);
}

function canConnect(host: string, port: number): Promise<boolean> {
  return new Promise((resolve) => {
    const socket = new Socket();

    const finish = (result: boolean) => {
      socket.destroy();
      resolve(result);
    };

    socket.setTimeout(1000);
    socket.once("connect", () => finish(true));
    socket.once("timeout", () => finish(false));
    socket.once("error", () => finish(false));
    socket.connect(port, host);
  });
}
