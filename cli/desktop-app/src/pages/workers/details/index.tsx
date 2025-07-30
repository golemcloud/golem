import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { formatRelativeTime } from "@/lib/utils";
import { API } from "@/service";
import { WSS } from "@/service/wss";
import {
  Invocation,
  OplogWithIndex,
  Terminal,
  Worker,
  WsMessage,
} from "@/types/worker.ts";
import { Activity, ActivityIcon, Clock, Cog, LayoutGrid } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { useParams } from "react-router-dom";
import { InvocationsChart } from "./widgets/invocationCharts";

export default function WorkerDetails() {
  const { componentId = "", workerName = "", appId } = useParams();
  const [workerDetails, setWorkerDetails] = useState({} as Worker);
  const wsRef = useRef<WSS | null>(null);
  const [invocationData, setInvocationData] = useState<Invocation[]>([]);
  const [terminal, setTerminal] = useState<Terminal[]>([]);

  useEffect(() => {
    if (componentId && workerName) {
      API.workerService
        .getParticularWorker(appId!, componentId, workerName)
        .then(response => {
          setWorkerDetails(response as Worker);
        });
    }
  }, [componentId, workerName]);

  useEffect(() => {
    async function fetchData() {
      setInvocationData([]);
      setTerminal([]);
      await getOpLog();
      const initWebSocket = async () => {
        try {
          const url = `/v1/components/${componentId}/workers/${workerName}/connect`;
          const ws = await WSS.getConnection(url);
          wsRef.current = ws;

          ws.onMessage((data: unknown) => {
            const message = data as WsMessage;
            if (message["InvocationStart"]) {
              setInvocationData(prev => [
                ...prev,
                {
                  timestamp: message.InvocationStart.timestamp,
                  function: message.InvocationStart.function,
                },
              ]);
            } else if (message["StdOut"]) {
              const bytes = message.StdOut.bytes || [];
              setTerminal(prev => [
                ...prev,
                {
                  timestamp: message.StdOut.timestamp,
                  message: String.fromCharCode(...bytes),
                },
              ]);
            }
          });
        } catch (error) {
          console.error("Failed to connect WebSocket:", error);
        }
      };

      initWebSocket();
    }

    fetchData();

    return () => {
      if (wsRef.current) {
        wsRef.current.close();
      }
    };
  }, []);

  const getOpLog = async () => {
    API.workerService
      .getOplog(appId!, componentId, workerName, "")
      .then(response => {
        const terminalData = [] as Terminal[];
        const invocationList = [] as Invocation[];
        (response as OplogWithIndex[]).forEach((_item: OplogWithIndex) => {
          const item = _item[1];
          if (item.type === "ExportedFunctionInvoked") {
            invocationList.push({
              timestamp: item.timestamp,
              function: item.functionName,
            });
          } else {
            terminalData.push({
              timestamp: item.timestamp,
              message: item.type,
            });
          }
        });
        setInvocationData(invocationList);
        setTerminal(terminalData);
      });
  };

  return (
    <div className="flex  space-y-6 h-[88vh] w-full overflow-y-auto">
      <div className="m-10 max-w-7xl mx-auto grid gap-10 ">
        <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-4">
          <Card>
            <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2 gap-4">
              <CardTitle className="text-sm font-medium">Status</CardTitle>
              <Activity className="h-4 w-4 text-muted-foreground" />
            </CardHeader>
            <CardContent>
              <div className="text-2xl font-bold">{workerDetails.status}</div>
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
                {(workerDetails.totalLinearMemorySize / (1024 * 1024)).toFixed(
                  2,
                )}{" "}
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
            <div className="w-full min-h-[150px]">
              {invocationData.length > 0 ? (
                <InvocationsChart data={invocationData} />
              ) : (
                <div className="flex flex-col items-center justify-center h-64 border-2 border-dashed border-gray-300 rounded-lg">
                  <LayoutGrid className="h-12 w-12 text-gray-400 mb-4" />
                  <h2 className="text-lg font-medium text-gray-600">
                    No Invocations was initiated
                  </h2>
                  <p className="text-sm text-gray-400">
                    Initiate an invocation to get started.
                  </p>
                </div>
              )}
            </div>
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle>Log</CardTitle>
          </CardHeader>
          <CardContent>
            <div>
              {terminal.length > 0 ? (
                <div className="bg-background border rounded-md p-4 font-mono text-sm space-y-2 min-h-[200px]">
                  {terminal.map(message => (
                    <div key={message.timestamp} className="border-b">
                      {message.message}
                    </div>
                  ))}
                </div>
              ) : (
                <div className="flex flex-col items-center justify-center h-64 border-2 border-dashed border-gray-300 rounded-lg">
                  <ActivityIcon className="h-12 w-12 text-gray-400 mb-4" />
                  <h2 className="text-lg font-medium text-gray-600">
                    No Terminal Output
                  </h2>
                  <p className="text-sm text-gray-400">
                    Initiate an invocation to get started and add terminal
                    output.
                  </p>
                </div>
              )}
            </div>
          </CardContent>
        </Card>
      </div>
    </div>
  );
}
