import { useEffect, useState } from "react";
import { LayoutGrid, Plus, Search } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Badge } from "@/components/ui/badge";
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from "@/components/ui/card.tsx";
import { useNavigate, useParams } from "react-router-dom";
import { Worker } from "@/types/worker.ts";
import { API } from "@/service";

const WORKER_COLOR_MAPPER = {
  Idle: "text-emerald-400 dark:text-emerald-200",
  Running: "text-blue-400 dark:text-blue-200",
  Suspended: "text-amber-400 dark:text-amber-200",
  Failed: "text-rose-400 dark:text-rose-200",
};

export default function WorkerList() {
  const [workerList, setWorkerList] = useState<Worker[]>([]);
  const [filteredWorkers, setFilteredWorkers] = useState<Worker[]>([]);
  const [searchQuery, setSearchQuery] = useState("");

  const navigate = useNavigate();
  const { appId, componentId } = useParams();

  useEffect(() => {
    API.workerService.findWorker(appId!, componentId!).then(res => {
      const sortedData = res.workers.sort(
        (a: Worker, b: Worker) =>
          new Date(a.createdAt).getTime() - new Date(b.createdAt).getTime(),
      );
      setWorkerList(sortedData);
      setFilteredWorkers(sortedData);
    });
  }, [componentId]);

  useEffect(() => {
    const lowerCaseQuery = searchQuery.toLowerCase();
    const filtered = workerList.filter(
      (worker: Worker) =>
        worker.workerName?.toLowerCase().includes(lowerCaseQuery) ||
        worker.status?.toLowerCase().includes(lowerCaseQuery),
    );
    setFilteredWorkers(filtered);
  }, [searchQuery, workerList]);

  return (
    <div className="flex">
      <div className="flex-1 p-8">
        <div className="p-6 bg-background text-foreground mx-auto max-w-7xl rounded-lg border border-border shadow-sm">
          <div className="flex gap-4 mb-4 items-center">
            <div className="relative flex-1">
              <Search className="absolute left-3 top-1/2 transform -translate-y-1/2 text-muted-foreground h-4 w-4" />
              <Input
                className="w-full pl-10 bg-muted rounded-md"
                placeholder="Search workers..."
                value={searchQuery}
                onChange={e => setSearchQuery(e.target.value)}
              />
            </div>
            <Button
              variant="default"
              onClick={() =>
                navigate(
                  `/app/${appId}/components/${componentId}/workers/create`,
                )
              }
            >
              <Plus className="h-4 w-4" />
              New Worker
            </Button>
          </div>

          {filteredWorkers.length === 0 ? (
            <div className="border-2 border-dashed border-gray-300 dark:border-gray-700 rounded-lg p-12 flex flex-col items-center justify-center">
              <div className="h-16 w-16 bg-gray-100 dark:bg-gray-800 rounded-lg flex items-center justify-center mb-4">
                <LayoutGrid className="h-8 w-8 text-gray-400" />
              </div>
              <h2 className="text-xl font-semibold text-foreground">
                No Workers Found
              </h2>
              <p className="text-muted-foreground">
                Create a new worker to get started.
              </p>
            </div>
          ) : (
            <div className="overflow-auto max-h-[70vh] space-y-4">
              {filteredWorkers.map((worker: Worker, index) => (
                <Card
                  key={index}
                  className="rounded-lg border border-border bg-muted hover:bg-muted/80 hover:shadow-lg transition cursor-pointer"
                  onClick={() =>
                    navigate(
                      `/app/${appId}/components/${componentId}/workers/${worker.workerName}`,
                    )
                  }
                >
                  <CardHeader>
                    <CardTitle className="text-foreground font-mono">
                      {worker.workerName}
                    </CardTitle>
                  </CardHeader>
                  <CardContent className="py-2">
                    <div className="grid grid-cols-1 md:grid-cols-4 gap-4">
                      <div>
                        <div className="text-sm text-muted-foreground">
                          Status
                        </div>
                        <div
                          className={`text-lg font-semibold ${WORKER_COLOR_MAPPER[worker.status as keyof typeof WORKER_COLOR_MAPPER]}`}
                        >
                          {worker.status}
                        </div>
                      </div>
                      <div>
                        <div className="text-sm text-muted-foreground">
                          Memory
                        </div>
                        <div className="text-lg font-semibold">
                          {worker.totalLinearMemorySize / 1024} KB
                        </div>
                      </div>
                      <div>
                        <div className="text-sm text-muted-foreground">
                          Pending Invocations
                        </div>
                        <div className="text-lg font-semibold">
                          {worker.pendingInvocationCount}
                        </div>
                      </div>
                      <div>
                        <div className="text-sm text-muted-foreground">
                          Version
                        </div>
                        <div className="text-lg font-semibold">
                          v{worker.componentVersion}
                        </div>
                      </div>
                    </div>
                    <div className="py-2 flex gap-2 text-sm">
                      <Badge variant="outline" className="rounded-sm">
                        Args: {worker.args.length}
                      </Badge>
                      <Badge variant="outline" className="rounded-sm">
                        Env: {Object.keys(worker.env).length}
                      </Badge>
                      <span className="text-muted-foreground ml-auto">
                        {new Date(worker.createdAt).toLocaleString()}
                      </span>
                    </div>
                  </CardContent>
                </Card>
              ))}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
