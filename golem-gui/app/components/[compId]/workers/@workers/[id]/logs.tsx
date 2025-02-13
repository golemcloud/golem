import React, { useState, useEffect, useCallback } from "react";
import { useWorkerLogs } from "@/lib/hooks/use-worker";
import { useCustomParam } from "@/lib/hooks/use-custom-param";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Input } from "@/components/ui/input";
import { Card, CardHeader, CardTitle, CardContent } from "@/components/ui/card";

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

interface OplogCursor {
  next_oplog_index: number;
  current_component_version: number;
}

// Custom debounce function
function debounce(func: (...args: any[]) => void, delay: number) {
  let timeoutId: NodeJS.Timeout;
  return function (...args: any[]) {
    clearTimeout(timeoutId);
    timeoutId = setTimeout(() => func.apply(null, args), delay);
  };
}

export default function WorkerLogs({lastClearTimeStamp}: {lastClearTimeStamp: Date | null}) {
  const { compId: componentId } = useCustomParam();
  const { id: workerName } = useCustomParam();

  // State for count, query, from, and cursor
  const [count, setCount] = useState<number>(10);
  const [query, setQuery] = useState<string>("");
  const [from, setFrom] = useState<string>("");
  const [cursor, setCursor] = useState<OplogCursor | null>(null); // Cursor for pagination
  const [debouncedQuery, setDebouncedQuery] = useState<string>("");

  // Fetch logs using the useWorkerLogs hook
  const { logs, error, isLoading } = useWorkerLogs(componentId, workerName, {
    count,
    query: debouncedQuery || undefined,
    from: Number(from) || undefined,
    cursor: cursor || undefined,
  });

  const entries = logs?.entries || [];

  // Debounce the search input
  const handleQueryChange = useCallback(
    debounce((query: string) => {
      setDebouncedQuery(query);
    }, 500),
    []
  );

  useEffect(() => {
    handleQueryChange(query);
  }, [query, handleQueryChange]);

  // Reset cursor when filters change
  useEffect(() => {
    setCursor(null);
  }, [count, debouncedQuery, from]);

  if (isLoading) {
    return <div>Loading logs...</div>;
  }

  if (error) {
    return <div>Error fetching logs: {error.message}</div>;
  }

  return (
    <div style={{ padding: "20px", fontFamily: "monospace" }}>
      <Card className='w-full max-w-6xl mx-auto'>
        <CardHeader>
          <CardTitle className='text-2xl font-mono'>Worker Logs</CardTitle>
          <p className='text-sm text-muted-foreground font-mono'>
            Component: {componentId} | Worker: {workerName}
          </p>
        </CardHeader>
        <CardContent>
          <div className='grid grid-cols-1 md:grid-cols-3 gap-4 mb-6'>
            <div className='space-y-2'>
              <label className='text-sm font-medium'>Show:</label>
              <Select
                value={count.toString()}
                onValueChange={(value) => setCount(Number(value))}
              >
                <SelectTrigger>
                  <SelectValue placeholder='Select count' />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value='10'>10 entries</SelectItem>
                  <SelectItem value='20'>20 entries</SelectItem>
                  <SelectItem value='50'>50 entries</SelectItem>
                  <SelectItem value='100'>100 entries</SelectItem>
                </SelectContent>
              </Select>
            </div>

            <div className='space-y-2'>
              <label className='text-sm font-medium'>Filter:</label>
              <Input
                type='text'
                placeholder='Search logs...'
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                className='font-mono'
              />
            </div>

            <div className='space-y-2'>
              <label className='text-sm font-medium'>From:</label>
              <Input
                type='datetime-local'
                value={from}
                onChange={(e) => setFrom(e.target.value)}
                className='font-mono'
              />
            </div>
          </div>
        </CardContent>
      </Card>

      {/* Log Entries */}
      <div className="mt-5">
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
                  {"          "}component version:{" "}
                  {entry.entry.component_version}
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
              {entry.entry.initial_active_plugins &&
                Array.isArray(entry.entry.initial_active_plugins) && (
                  <>
                    {"          "}initial active plugins:
                    {entry.entry.initial_active_plugins.map((plugin, idx) => (
                      <div key={idx}>
                        {"            "}- {plugin.plugin_name} (v
                        {plugin.plugin_version})
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
                  {"          "}response:{" "}
                  {JSON.stringify(entry.entry.response.value)}
                  {"\n"}
                </>
              )}
            </pre>
          </div>
        ))}
      </div>

      {logs?.next && (
        <button
          onClick={() => setCursor(logs.next)}
          style={{ marginTop: "20px" }}
        >
          ...Load More
        </button>
      )}
    </div>
  );
}
