import React from "react";
import { useWorkerLogs } from "@/lib/hooks/use-worker";
import { useCustomParam } from "@/lib/hooks/use-custom-param";

interface WorkerLogEntry {
  oplogIndex: number;
  entry: {
    type: string;
    timestamp: string;
    component_version?: number;
    args?: any[];
    env?: Record<string, string>;
    initial_active_plugins?: Array<{
      plugin_name: string;
      plugin_version: string;
    }>;
    component_size?: number;
    initial_total_linear_memory_size?: number;
    parent?: {
      componentId: string;
      workerName: string;
    };
    worker_id?: {
      componentId: string;
      workerName: string;
    };
    function_name?: string;
    idempotency_key?: string;
    request?: Array<{
      typ: {
        type: string;
      };
      value: any;
    }>;
    response?: {
      typ: {
        err?: {
          cases: Array<{
            name: string;
            typ: any;
          }>;
        };
      };
      value: any;
    };
  };
}

export default function WorkerLogs() {
  const { compId: componentId } = useCustomParam();
  const { id: workerName } = useCustomParam();

  const { logs, error, isLoading } = useWorkerLogs(componentId, workerName, {
    count: 100,
  });

  const entries = logs?.entries || [];

  if (isLoading) {
    return <div>Loading logs...</div>;
  }

  if (error) {
    return <div>Error fetching logs: {error.message}</div>;
  }

  return (
    <div style={{ padding: "20px", fontFamily: "monospace" }}>
      <h1>Worker Logs</h1>
      <h2>
        Component: {componentId} | Worker: {workerName}
      </h2>
      <div>
        {entries.map((entry: WorkerLogEntry, index: number) => (
          <div
            key={index}
            style={{
              marginBottom: "20px",
              borderBottom: "1px solid #ccc",
              padding: "10px",
            }}
          >
            <pre
              style={{
                whiteSpace: "pre-wrap",
                wordBreak: "break-word",
                overflowWrap: "break-word",
                fontFamily: "monospace",
              }}
            >
              #{entry.oplogIndex}: {entry.entry.type}
              {"\n"}
              {"          "}at: {entry.entry.timestamp}
              {"\n"}
              {entry.entry.component_version !== undefined && (
                <>
                  {"          "}component version: {entry.entry.component_version}
                  {"\n"}
                </>
              )}
              {entry.entry.args && (
                <>
                  {"          "}args: {JSON.stringify(entry.entry.args)}
                  {"\n"}
                </>
              )}
              {entry.entry.env && (
                <>
                  {"          "}env:
                  {Object.entries(entry.entry.env).map(([key, value]) => (
                    <div key={key}>
                      {"            "}- {key}: {value}
                    </div>
                  ))}
                  {"\n"}
                </>
              )}
              {entry.entry.initial_active_plugins && Array.isArray(entry.entry.initial_active_plugins) && (
                <>
                  {"          "}initial active plugins:
                  {entry.entry.initial_active_plugins.map((plugin, idx) => (
                    <div key={idx}>
                      {"            "}- {plugin.plugin_name} (v{plugin.plugin_version})
                    </div>
                  ))}
                  {"\n"}
                </>
              )}
              {entry.entry.function_name && (
                <>
                  {"          "}function name: {entry.entry.function_name}
                  {"\n"}
                </>
              )}
              {entry.entry.idempotency_key && (
                <>
                  {"          "}idempotency key: {entry.entry.idempotency_key}
                  {"\n"}
                </>
              )}
              {entry.entry.request && Array.isArray(entry.entry.request) && (
                <>
                  {"          "}request:
                  {entry.entry.request.map((req, idx) => (
                    <div key={idx}>
                      {"            "}- {req.value} (type: {req.typ.type})
                    </div>
                  ))}
                  {"\n"}
                </>
              )}
              {entry.entry.response && (
                <>
                  {"          "}response: {JSON.stringify(entry.entry.response.value)}
                  {"\n"}
                </>
              )}
            </pre>
          </div>
        ))}
      </div>
    </div>
  );
}