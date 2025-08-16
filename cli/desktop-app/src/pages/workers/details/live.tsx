import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
// import {
//   Select,
//   SelectContent,
//   SelectItem,
//   SelectTrigger,
// } from "@/components/ui/select";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { useDebounce } from "@/hooks/debounce"; // Import the "debounce" hook
import { formatTimestampInDateTimeFormat } from "@/lib/utils";
import { API } from "@/service";
import { WSS } from "@/service/wss";
import {
  Invocation,
  OplogWithIndex,
  Terminal,
  WsMessage,
} from "@/types/worker.ts";
import { RotateCw, Search, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { useParams } from "react-router-dom";

export default function WorkerLive() {
  const { componentId = "", workerName = "", appId } = useParams();
  const wsRef = useRef<WSS | null>(null);
  const [invocationData, setInvocationData] = useState<Invocation[]>([]);
  const [terminal, setTerminal] = useState<Terminal[]>([]);
  const [activeTab, setActiveTab] = useState("log");
  // const [count, setCount] = useState("100");
  const [searchQuery, setSearchQuery] = useState("");

  // Debounced values to prevent rapid API calls
  const debouncedSearchQuery = useDebounce(searchQuery, 300);
  const debouncedActiveTab = useDebounce(activeTab, 300);

  useEffect(() => {
    async function fetchData() {
      setInvocationData([]);
      setTerminal([]);
      await getOpLog(debouncedSearchQuery);

      const initWebSocket = async () => {
        try {
          const ws = await WSS.getConnection(
            `/v1/components/${componentId}/workers/${workerName}/connect`,
          );
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
      wsRef.current?.close();
    };
  }, []);

  useEffect(() => {
    getOpLog(debouncedSearchQuery);
  }, [debouncedActiveTab, debouncedSearchQuery]);

  const getOpLog = async (search: string) => {
    API.workerService
      .getOplog(
        appId!,
        componentId,
        workerName,
        `${
          debouncedActiveTab === "log" ? "" : "ExportedFunctionInvoked"
        } ${search}`,
      )
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
    <div className="flex flex-col">
      <div className="space-y-6 overflow-scroll h-[87vh]">
        <div className="w-full bg-background">
          <Tabs
            value={activeTab}
            onValueChange={setActiveTab}
            className="w-full"
          >
            <div className="flex items-center justify-between border-b border-border/40 px-2">
              <TabsList className="h-12 bg-transparent p-0">
                <TabsTrigger
                  value="log"
                  className="relative h-12 rounded-none border-b-2 border-transparent px-4 pb-3 pt-3 font-medium text-muted-foreground hover:text-foreground data-[state=active]:border-primary data-[state=active]:text-foreground"
                >
                  Log
                </TabsTrigger>
                <TabsTrigger
                  value="invocations"
                  className="relative h-12 rounded-none border-b-2 border-transparent px-4 pb-3 pt-3 font-medium text-muted-foreground hover:text-foreground data-[state=active]:border-primary data-[state=active]:text-foreground"
                >
                  Invocations
                </TabsTrigger>
              </TabsList>
              <div className="flex gap-2 pr-2 items-center">
                <div className="relative flex-1 max-full">
                  <Search className="absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
                  <Input
                    placeholder="Search..."
                    className="pl-9"
                    value={searchQuery}
                    onChange={e => setSearchQuery(e.target.value)}
                  />
                  {searchQuery && (
                    <button
                      onClick={() => setSearchQuery("")}
                      className="absolute right-3 top-1/2 -translate-y-1/2 text-gray-500 hover:text-gray-700"
                    >
                      <X size={18} />
                    </button>
                  )}
                </div>
                {/*<Select defaultValue={count} onValueChange={e => setCount(e)}>*/}
                {/*  <SelectTrigger className="w-[80px]">{count}</SelectTrigger>*/}
                {/*  <SelectContent>*/}
                {/*    <SelectItem value={"10"}>10</SelectItem>*/}
                {/*    <SelectItem value={"25"}>25</SelectItem>*/}
                {/*    <SelectItem value={"50"}>50</SelectItem>*/}
                {/*    <SelectItem value={"75"}>75</SelectItem>*/}
                {/*    <SelectItem value={"100"}>100</SelectItem>*/}
                {/*  </SelectContent>*/}
                {/*</Select>*/}
                <Button
                  variant="ghost"
                  size="sm"
                  className="h-8 text-destructive hover:bg-destructive/10 hover:text-destructive"
                  onClick={() => {
                    setInvocationData([]);
                    setTerminal([]);
                  }}
                >
                  <X className="h-4 w-4 mr-1.5" />
                  Clear
                </Button>
                <Button
                  variant="ghost"
                  size="sm"
                  className="h-8 text-primary hover:bg-primary/10 hover:text-primary"
                  onClick={() => getOpLog(searchQuery)}
                >
                  <RotateCw className="h-4 w-4 mr-1.5" />
                  Reload
                </Button>
              </div>
            </div>

            <TabsContent value="log" className="m-0">
              <div className="p-4 font-mono text-sm">
                {terminal.length > 0 ? (
                  terminal.map(log => (
                    <div
                      key={log.timestamp}
                      className="group flex py-1 hover:bg-muted/50 border-b"
                    >
                      <span className="shrink-0 text-muted-foreground/70">
                        {formatTimestampInDateTimeFormat(log.timestamp)}
                      </span>
                      <span className="ml-4 text-foreground">
                        {log.message}
                      </span>
                    </div>
                  ))
                ) : (
                  <div className="p-4 text-center text-muted-foreground">
                    No terminal content
                  </div>
                )}
              </div>
            </TabsContent>

            <TabsContent value="invocations" className="m-0">
              <div className="p-4 font-mono text-sm">
                {invocationData.length > 0 ? (
                  invocationData.map(log => (
                    <div
                      key={log.timestamp}
                      className="group flex py-1 hover:bg-muted/50 border-b"
                    >
                      <span className="shrink-0 text-muted-foreground/70">
                        {formatTimestampInDateTimeFormat(log.timestamp)}
                      </span>
                      <span className="ml-4 text-foreground">
                        {log.function}
                      </span>
                    </div>
                  ))
                ) : (
                  <div className="p-4 text-center text-muted-foreground">
                    No invocations content
                  </div>
                )}
              </div>
            </TabsContent>
          </Tabs>
        </div>
      </div>
    </div>
  );
}
