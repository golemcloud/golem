/* eslint-disable @typescript-eslint/no-explicit-any */
import ErrorBoundary from "@/components/errorBoundary";
import WorkerLeftNav from "./leftNav";
import { API } from "@/service";
import { Invocation, Terminal, Worker } from "@/types/worker.ts";
import { useEffect, useState, useRef } from "react";
import { useParams } from "react-router-dom";
import { Activity, Clock, Cog } from "lucide-react";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { InvocationsChart } from "./widgets/invocationCharts";
import { formatRelativeTime } from "@/lib/utils";
import { WSS } from "@/service/wss";

export default function WorkerDetails() {
  const { componentId, workerName } = useParams();
  const [workerDetails, setWorkerDetails] = useState({} as Worker);
  const wsRef = useRef<WSS | null>(null);
  const [messages, setMessages] = useState<any[]>([]);

  useEffect(() => {
    if (componentId && workerName) {
      API.getParticularWorker(componentId, workerName).then((response) => {
        setWorkerDetails(response);
      });
    }
  }, [componentId, workerName]);

  useEffect(() => {
    const initWebSocket = async () => {
      try {
        const ws = await WSS.getConnection(
          `ws://localhost:9881/v1/components/${componentId}/workers/${workerName}/connect`
        );
        wsRef.current = ws;

        ws.onMessage((data) => {
          setMessages((prev) => [...prev, data]); // Update messages state
        });
      } catch (error) {
        console.error("Failed to connect WebSocket:", error);
      }
    };

    initWebSocket();

    return () => {
      if (wsRef.current) {
        wsRef.current.close();
      }
    };
  }, []);

  const invocationData = [] as Invocation[];
  const terminal = [] as Terminal[];
  messages.forEach((message) => {
    const invocationStart = message["InvocationStart"];
    const stdOut = message["StdOut"];
    if (invocationStart)
      invocationData.push({
        timestamp: invocationStart.timestamp,
        function: invocationStart.function,
      });
    else if (stdOut) {
      terminal.push({
        timestamp: stdOut.timestamp,
        message: String.fromCharCode(...stdOut.bytes),
      });
    }
  });

  return (
    <ErrorBoundary>
      <div className="flex">
        <WorkerLeftNav />
        <div className="flex-1 flex flex-col">
          <header className="w-full border-b bg-background py-4">
            <div className="mx-auto px-6 lg:px-8">
              <div className="flex items-center gap-4">
                <h1 className="text-xl font-semibold text-foreground truncate">
                  {workerName}
                </h1>
              </div>
            </div>
          </header>
          <div className="p-10 space-y-6 max-w-7xl mx-auto overflow-scroll h-[76vh]">
            <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-4">
              <Card>
                <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2 gap-4">
                  <CardTitle className="text-sm font-medium">Status</CardTitle>
                  <Activity className="h-4 w-4 text-muted-foreground" />
                </CardHeader>
                <CardContent>
                  <div className="text-2xl font-bold">
                    {workerDetails.status}
                  </div>
                </CardContent>
              </Card>

              <Card>
                <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2 gap-4">
                  <CardTitle className="text-sm font-medium">
                    Memory Usage
                  </CardTitle>
                  <Cog className="h-4 w-4 text-muted-foreground " />
                </CardHeader>
                <CardContent>
                  <div className="text-2xl font-bold">
                    {(
                      workerDetails.totalLinearMemorySize /
                      (1024 * 1024)
                    ).toFixed(2)}{" "}
                    MB
                  </div>
                </CardContent>
              </Card>

              <Card>
                <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2 gap-4">
                  <CardTitle className="text-sm font-medium">
                    Resource Count
                  </CardTitle>
                  <Cog className="h-4 w-4 text-muted-foreground" />
                </CardHeader>
                <CardContent>
                  <div className="text-2xl font-bold">
                    {workerDetails.ownedResources &&
                      Object.keys(workerDetails.ownedResources).length}
                  </div>
                </CardContent>
              </Card>

              <Card>
                <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2 gap-4">
                  <CardTitle className="text-sm font-medium">Created</CardTitle>
                  <Clock className="h-4 w-4 text-muted-foreground" />
                </CardHeader>
                <CardContent>
                  <div className="text-2xl font-bold">
                    {formatRelativeTime(workerDetails.createdAt || new Date())}{" "}
                  </div>
                </CardContent>
              </Card>
            </div>

            <Card>
              <CardHeader>
                <CardTitle>Invocations</CardTitle>
              </CardHeader>
              <CardContent>
                <div className="w-full">
                  {invocationData.length > 0 ? (
                    <InvocationsChart data={invocationData} />
                  ) : (
                    <>No messages</>
                  )}
                </div>
              </CardContent>
            </Card>

            <Card>
              <CardHeader>
                <CardTitle>Terminal</CardTitle>
              </CardHeader>
              <CardContent>
                <div className="bg-background border rounded-md p-4 font-mono text-sm space-y-2">
                  {terminal.length > 0 ? (
                    terminal.map((message) => (
                      <div key={message.timestamp} className="border-b">
                        {message.message}
                      </div>
                    ))
                  ) : (
                    <>No messages</>
                  )}
                </div>
              </CardContent>
            </Card>
          </div>
        </div>
      </div>
    </ErrorBoundary>
  );
}
