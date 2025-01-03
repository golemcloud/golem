import { useState, useEffect } from "react";
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
import ErrorBoundary from "@/components/errorBoundary";
import ComponentLeftNav from "../components/details/componentsLeftNav";
import { Worker } from "@/types/worker.ts";
import { API } from "@/service";

export default function WorkerList() {
  const [workerList, setWorkerList] = useState([] as Worker[]);
  const [filteredWorkers, setFilteredWorkers] = useState([] as Worker[]);
  const [searchQuery, setSearchQuery] = useState("");

  const navigate = useNavigate();
  const { componentId } = useParams();

  const filterWorkers = () => {
    const lowerCaseQuery = searchQuery.toLowerCase();
    const filtered = workerList.filter(
      (worker) =>
        worker.workerId.workerName.toLowerCase().includes(lowerCaseQuery) ||
        worker.status.toLowerCase().includes(lowerCaseQuery)
    );
    setFilteredWorkers(filtered);
  };

  useEffect(() => {
    API.findWorker(componentId!).then((res) => {
      setWorkerList(res.workers);
      setFilteredWorkers(res.workers); // Initially, show all workers
    });
  }, [componentId]);

  useEffect(() => {
    filterWorkers();
  }, [searchQuery, workerList]);

  return (
    <ErrorBoundary>
      <div className="flex">
        <ComponentLeftNav />
        <div className="flex-1 flex flex-col">
          <header className="w-full border-b bg-background py-4">
            <div className="mx-auto px-6 lg:px-8">
              <div className="flex items-center gap-4">
                <h1 className="text-xl font-semibold text-foreground truncate">
                  {componentId}
                </h1>
              </div>
            </div>
          </header>
          <div className="flex-1 p-8">
            <div className="p-4 bg-background text-foreground mx-auto max-w-7xl p-10">
              <div className="flex gap-4 mb-4 items-center">
                <div className="relative flex-1">
                  <Search className="absolute left-3 top-1/2 transform -translate-y-1/2 text-muted-foreground h-4 w-4" />
                  <Input
                    className="w-full pl-10"
                    placeholder="Search by worker name or status..."
                    value={searchQuery}
                    onChange={(e) => setSearchQuery(e.target.value)}
                  />
                </div>
                <Button
                  variant="default"
                  onClick={() =>
                    navigate(`/components/${componentId}/workers/create`)
                  }
                >
                  <Plus className="h-4 w-4" />
                  New
                </Button>
              </div>

              {Object.keys(filteredWorkers).length === 0 ? (
                <div className="border-2 border-dashed border-gray-200 rounded-lg p-12 flex flex-col items-center justify-center">
                  <div className="h-16 w-16 bg-gray-100 rounded-lg flex items-center justify-center mb-4">
                    <LayoutGrid className="h-8 w-8 text-gray-400" />
                  </div>
                  <h2 className="text-xl font-semibold mb-2 text-center">
                    No Worker
                  </h2>
                  <p className="text-gray-500 mb-6 text-center">
                    Create a new worker to get started.
                  </p>
                </div>
              ) : (
                <div className="overflow-scroll h-[55vh]">
                  {filteredWorkers.map((worker, index) => (
                    <Card
                      key={index}
                      className="rounded-lg mb-4 cursor-pointer"
                      onClick={() =>
                        navigate(
                          `/components/${componentId}/workers/${worker.workerId.workerName}`
                        )
                      }
                    >
                      <CardHeader>
                        <div className="flex justify-between items-center">
                          <CardTitle>{worker.workerId.workerName}</CardTitle>
                        </div>
                      </CardHeader>
                      <CardContent className="py-2">
                        <div className="grid grid-cols-1 md:grid-cols-4 gap-4">
                          <div>
                            <div className="text-sm text-muted-foreground">
                              Status
                            </div>
                            <div className="flex items-center gap-1">
                              {worker.status}
                            </div>
                          </div>
                          <div>
                            <div className="text-sm text-muted-foreground">
                              Memory
                            </div>
                            <div className="flex items-center gap-1">
                              {worker.totalLinearMemorySize / 1024} KB
                            </div>
                          </div>
                          <div>
                            <div className="text-sm text-muted-foreground">
                              Pending Invocations
                            </div>
                            <div className="flex items-center gap-1">
                              {worker.pendingInvocationCount}
                            </div>
                          </div>
                          <div>
                            <div className="text-sm text-muted-foreground">
                              Version
                            </div>
                            <div className="flex items-center gap-1">
                              v{worker.componentVersion}
                            </div>
                          </div>
                        </div>
                        <div className="py-1 flex gap-2">
                          <Badge variant="outline" className="rounded-sm">
                            Args: {worker.args.length}
                          </Badge>
                          <Badge variant="outline" className="rounded-sm">
                            Env: {Object.keys(worker.env).length}
                          </Badge>
                          <span className="text-sm text-muted-foreground ml-auto">
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
      </div>
    </ErrorBoundary>
  );
}
