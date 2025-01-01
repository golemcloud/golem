/* eslint-disable @typescript-eslint/no-unused-vars */
import { useState, useEffect } from "react";
import { Search, LayoutGrid, PlusCircle } from "lucide-react";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";

import { useNavigate } from "react-router-dom";
import { Button } from "@/components/ui/button.tsx";
import { formatRelativeTime } from "@/lib/utils";
import { Input } from "@/components/ui/input";
import { SERVICE } from "@/service";
import { Component } from "@/types/component";
import { Worker, WorkerStatus } from "@/types/worker";

const Metrix = ["Idle", "Running", "Suspended", "Failed"];

const Components = () => {
  const navigate = useNavigate();
  const [componentList, setComponentList] = useState<{
    [key: string]: Component;
  }>({});
  const [componentApiList, setComponentApiList] = useState<{
    [key: string]: Component;
  }>({});
  const [workerList, setWorkerList] = useState(
    {} as {
      [key: string]: WorkerStatus;
    }
  );

  useEffect(() => {
    SERVICE.getComponents().then((response) => {
      const componentData = {} as { [key: string]: Component };
      response.forEach((data: Component) => {
        if (data?.versionedComponentId?.componentId) {
          componentData[data.versionedComponentId.componentId] = {
            componentName: data.componentName,
            componentId: data.versionedComponentId.componentId,
            createdAt: data.createdAt,
            exports: data?.metadata?.exports,
            componentSize: data.componentSize,
            componentType: data.componentType,
            versionId: [
              ...(componentData[data.versionedComponentId.componentId]
                ?.versionId || []),
              data.versionedComponentId.version,
            ],
          };
        }
      });
      setComponentApiList(componentData);
      setComponentList(componentData);
    });
  }, []);

  useEffect(() => {
    SERVICE.getWorkers().then((response) => {
      const workerData = {} as {
        [key: string]: WorkerStatus;
      };
      response.workers.forEach((data: Worker) => {
        const exisitngData = workerData[data.workerId.componentId] || {};
        switch (data.status) {
          case "Idle":
            if (exisitngData.Idle) {
              exisitngData.Idle++;
            } else {
              exisitngData["Idle"] = 1;
            }
            break;
          case "Running":
            if (exisitngData.Running) {
              exisitngData.Running++;
            } else {
              exisitngData["Running"] = 1;
            }
            break;
          case "Suspended":
            if (exisitngData.Suspended) {
              exisitngData.Suspended++;
            } else {
              exisitngData["Suspended"] = 1;
            }
            break;
          case "Failed":
            if (exisitngData.Failed) {
              exisitngData.Failed++;
            } else {
              exisitngData["Failed"] = 1;
            }
            break;
          default:
        }
        workerData[data.workerId.componentId] = exisitngData;
      });
      setWorkerList(workerData);
    });
  }, []);

  const handleSearch = (e: React.ChangeEvent<HTMLInputElement>) => {
    const value = e.target.value;
    const filteredList = Object.fromEntries(
      Object.entries(componentApiList).filter(
        ([_, data]: [string, Component]) =>
          data.componentName?.toLowerCase().includes(value) ?? false
      )
    );

    setComponentList(filteredList);
  };

  return (
    <div className="container mx-auto px-4 py-8">
      <div className="flex flex-wrap items-center justify-between gap-4 mb-8">
        <div className="relative flex-1">
          <Search className="absolute left-3 top-1/2 transform -translate-y-1/2 text-gray-400 h-5 w-5" />
          <Input
            type="text"
            placeholder="Search Components..."
            className="w-full pl-10 pr-4 py-2"
            onChange={(e) => handleSearch(e)}
          />
        </div>
        <div className="flex items-center gap-2">
          <Button onClick={() => navigate("/components/create")}>
            <PlusCircle className="mr-2 size-4" />
            Create Component
          </Button>
        </div>
      </div>

      {Object.keys(componentList).length === 0 ? (
        <div className="border-2 border-dashed border-gray-200 rounded-lg p-12 flex flex-col items-center justify-center">
          <div className="h-16 w-16 bg-gray-100 rounded-lg flex items-center justify-center mb-4">
            <LayoutGrid className="h-8 w-8 text-gray-400" />
          </div>
          <h2 className="text-xl font-semibold mb-2 text-center">
            No Project Components
          </h2>
          <p className="text-gray-500 mb-6 text-center">
            Create a new component to get started.
          </p>
        </div>
      ) : (
        <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-6 overflow-scroll max-h-[78vh]">
          {Object.values(componentList).map((data: Component) => (
            <Card
              key={data.componentId}
              className="border shadow-sm cursor-pointer"
              onClick={() => navigate(`/components/${data.componentId}`)}
            >
              <CardHeader className="pb-4">
                <CardTitle className="text-lg font-medium">
                  {data.componentName}
                </CardTitle>
              </CardHeader>
              <CardContent className="space-y-4">
                <div className="grid grid-cols-2 sm:grid-cols-4 :grid-cols-4  gap-2">
                  {Metrix.map((metric) => (
                    <div
                      key={metric}
                      className="flex flex-col items-start space-y-1"
                    >
                      <span className="text-sm text-muted-foreground">
                        {metric}
                      </span>
                      <span className="text-lg font-medium">
                        {data.componentId !== undefined
                          ? (
                              workerList[data.componentId] as unknown as Record<
                                string,
                                number
                              >
                            )?.[metric] || 0
                          : 0}
                      </span>
                    </div>
                  ))}
                </div>
                <div className="flex flex-wrap items-center gap-2">
                  <Badge variant="secondary" className="rounded-md">
                    V{data.versionId?.[0]}
                  </Badge>
                  <Badge variant="secondary" className="rounded-md">
                    {data.exports?.[0]?.functions.length || 0} Exports
                  </Badge>
                  <Badge variant="secondary" className="rounded-md">
                    {Math.round((data.componentSize || 0) / 1024)} KB
                  </Badge>
                  <Badge variant="secondary" className="rounded-md">
                    {data.componentType}
                  </Badge>
                  <span className="ml-auto text-sm text-muted-foreground">
                    {formatRelativeTime(data.createdAt || new Date())}
                  </span>
                </div>
              </CardContent>
            </Card>
          ))}
        </div>
      )}
    </div>
  );
};

export default Components;
