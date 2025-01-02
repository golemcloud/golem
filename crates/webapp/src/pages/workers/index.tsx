import { useState, useEffect } from "react";
import { Plus, Search } from "lucide-react";
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

          <div className=" overflow-scroll h-[80vh]">
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
        </div>
      </div>
    </ErrorBoundary>
  );
}
