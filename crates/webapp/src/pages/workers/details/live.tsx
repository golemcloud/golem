/* eslint-disable @typescript-eslint/no-explicit-any */
import ErrorBoundary from "@/components/errorBoundary";
import WorkerLeftNav from "./leftNav";
import { Invocation, Terminal } from "@/types/worker.ts";
import { useEffect, useState, useRef } from "react";
import { useParams } from "react-router-dom";
import { WSS } from "@/service/wss";
import { Button } from "@/components/ui/button";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { RotateCw, X } from "lucide-react";
import { ScrollArea } from "@/components/ui/scroll-area";
import { formatTimestampInDateTimeFormat } from "@/lib/utils";

export default function WorkerLive() {
  const { componentId, workerName } = useParams();
  const wsRef = useRef<WSS | null>(null);
  const [messages, setMessages] = useState<any[]>([]);
  const [reload, setReload] = useState(false);

  useEffect(() => {
    setMessages([]);
    const initWebSocket = async () => {
      try {
        const ws = await WSS.getConnection(
          `ws://localhost:9881/v1/components/${componentId}/workers/${workerName}/connect`
        );
        wsRef.current = ws;

        ws.onMessage((data) => {
          setMessages((prev) => [...prev, data]);
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
  }, [reload]);

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
          <div className="space-y-6 overflow-scroll h-[70vh]">
            <div className="w-full bg-background">
              <Tabs defaultValue="terminal" className="w-full">
                <div className="flex items-center justify-between border-b border-border/40 px-2">
                  <TabsList className="h-12 bg-transparent p-0">
                    <TabsTrigger
                      value="terminal"
                      className="relative h-12 rounded-none border-b-2 border-transparent px-4 pb-3 pt-3 font-medium text-muted-foreground hover:text-foreground data-[state=active]:border-primary data-[state=active]:text-foreground"
                    >
                      Terminal
                    </TabsTrigger>
                    <TabsTrigger
                      value="invocations"
                      className="relative h-12 rounded-none border-b-2 border-transparent px-4 pb-3 pt-3 font-medium text-muted-foreground hover:text-foreground data-[state=active]:border-primary data-[state=active]:text-foreground"
                    >
                      Invocations
                    </TabsTrigger>
                  </TabsList>
                  <div className="flex gap-2 pr-2">
                    <Button
                      variant="ghost"
                      size="sm"
                      className="h-8 text-destructive hover:bg-destructive/10 hover:text-destructive"
                      onClick={() => setMessages([])}
                    >
                      <X className="h-4 w-4 mr-1.5" />
                      Clear
                    </Button>
                    <Button
                      variant="ghost"
                      size="sm"
                      className="h-8 text-primary hover:bg-primary/10 hover:text-primary"
                      onClick={() => setReload(!reload)}
                    >
                      <RotateCw className="h-4 w-4 mr-1.5" />
                      Reload
                    </Button>
                  </div>
                </div>

                <TabsContent value="terminal" className="m-0">
                  <ScrollArea className="h-[600px] w-full">
                    <div className="p-4 font-mono text-sm">
                      {terminal.length > 0 ? (
                        terminal.map((log) => (
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
                  </ScrollArea>
                </TabsContent>

                <TabsContent value="invocations" className="m-0">
                  <div className="p-4 font-mono text-sm">
                    {invocationData.length > 0 ? (
                      invocationData.map((log) => (
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
      </div>
    </ErrorBoundary>
  );
}
