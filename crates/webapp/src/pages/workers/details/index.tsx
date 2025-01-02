import ErrorBoundary from "@/components/errorBoundary";
import WorkerLeftNav from "./leftNav";
import { API } from "@/service";
import { Worker } from "@/types/worker.ts";
import { useEffect, useState } from "react";
import { useParams } from "react-router-dom";
import { Activity, Clock, Cog } from "lucide-react";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { InvocationsChart } from "./widgets/invocationCharts";
import { formatRelativeTime } from "@/lib/utils";

export default function WorkerDetails() {
  const { componentId, workerName } = useParams();
  const [workerDetails, setWorkerDetails] = useState({} as Worker);

  useEffect(() => {
    if (componentId && workerName) {
      API.getParticularWorker(componentId, workerName).then((response) => {
        setWorkerDetails(response);
      });
    }
  }, [componentId, workerName]);

  console.log(workerDetails, "workerDetails");

  return (
    <ErrorBoundary>
      <div className="flex">
        <WorkerLeftNav />
        <div className="p-6 space-y-6 max-w-7xl mx-auto overflow-scroll h-[88vh]">
          <div className="flex items-center gap-2">
            <h1 className="text-lg font-mono text-muted-foreground">
              {workerName}
            </h1>
          </div>

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
              <div className="h-[200px] w-full">
                <InvocationsChart />
              </div>
            </CardContent>
          </Card>

          <Card>
            <CardHeader>
              <CardTitle>Terminal</CardTitle>
            </CardHeader>
            <CardContent>
              <div className="bg-background border rounded-md p-4 font-mono text-sm space-y-2">
                <div className="border-b">
                  Initializing cart for user fgfgfg
                </div>
                <div className="border-b">
                  Initializing cart for user fgfgfg
                </div>
                <div className="border-b">
                  Initializing cart for user fgfgfg
                </div>
                <div className="border-b">Initializing cart for user</div>
              </div>
            </CardContent>
          </Card>
        </div>
      </div>
    </ErrorBoundary>
  );
}
