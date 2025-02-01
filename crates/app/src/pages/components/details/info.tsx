import { useParams } from "react-router-dom";

import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Download } from "lucide-react";
import ComponentLeftNav from "./componentsLeftNav";
import ErrorBoundary from "@/components/errorBoundary";
import { writeFile } from "@tauri-apps/plugin-fs";
import { BaseDirectory } from "@tauri-apps/api/path";
import { useEffect, useState } from "react";
import { Component } from "@/types/component";
import { API } from "@/service";
import { formatRelativeTime } from "@/lib/utils";
import { toast } from "@/hooks/use-toast";

export default function ComponentInfo() {
  const { componentId } = useParams();
  const [componentList, setComponentList] = useState([] as Component[]);
  const [component, setComponent] = useState<Component>({});
  const [versionList, setVersionList] = useState([] as number[]);
  const [versionChange, setVersionChange] = useState("0" as string);

  useEffect(() => {
    if (componentId) {
      API.getComponentByIdAsKey().then((response) => {
        setVersionList(response[componentId].versionId || []);
      });

      API.getComponents().then((response) => {
        setComponentList(response);
        const selectedComponentList = response.filter(
          (component: Component) =>
            component.versionedComponentId?.componentId === componentId
        );
        let mostRecentComponent = {} as Component;
        selectedComponentList.forEach((component: Component) => {
          if (component.createdAt) {
            const currentDate = new Date(component.createdAt);
            if (
              !mostRecentComponent.createdAt ||
              currentDate > new Date(mostRecentComponent.createdAt)
            ) {
              mostRecentComponent = component;
            }
          }
        });
        setComponent(mostRecentComponent);
        setVersionChange(
          mostRecentComponent?.versionedComponentId?.version?.toString() || ""
        );
      });
    }
  }, [componentId]);

  const handleVersionChange = (version: string) => {
    setVersionChange(version);
    const componentDetails = componentList.find((component: Component) => {
      if (component.versionedComponentId) {
        return (
          component.versionedComponentId.componentId === componentId &&
          component.versionedComponentId.version?.toString() === version
        );
      }
    });
    setComponent(componentDetails || {});
  };

  async function downloadFile() {
    try {
      API.downloadComponent(componentId!, versionChange).then(
        async (response) => {
          const blob = await response.blob();
          const arrayBuffer = await blob.arrayBuffer();

          // Specify the file name and path
          const fileName = `${componentId}.wasm`;
          await writeFile(fileName, new Uint8Array(arrayBuffer), {
            baseDir: BaseDirectory.Download,
          });
          toast({
            title: "File downloaded successfully",
            duration: 3000,
          });
        }
      );
    } catch (error) {
      console.error("Error downloading the file:", error);
    }
  }

  return (
    <ErrorBoundary>
      <div className="flex">
        <ComponentLeftNav componentDetails={component} />
        <div className="flex-1 flex flex-col">
          <header className="w-full border-b bg-background py-4">
            <div className="mx-auto px-6 lg:px-8">
              <div className="flex items-center gap-4">
                <h1 className="text-xl font-semibold text-foreground truncate">
                  {component.componentName}
                </h1>
              </div>
            </div>
          </header>
          <div className="flex-1 p-8">
            <Card className="w-full max-w-3xl mx-auto">
              <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-7">
                <div className="space-y-1.5">
                  <CardTitle className="text-2xl font-semibold">
                    Component Information
                  </CardTitle>
                  <CardDescription>
                    View metadata about this component
                  </CardDescription>
                </div>
                <div className="flex items-center gap-2">
                  {versionList.length > 0 && (
                    <Select
                      defaultValue={versionChange}
                      onValueChange={(version) => handleVersionChange(version)}
                    >
                      <SelectTrigger className="w-[80px]">
                        <SelectValue> v{versionChange}</SelectValue>
                      </SelectTrigger>
                      <SelectContent>
                        {versionList.map((version: number) => (
                          <SelectItem key={version} value={String(version)}>
                            v{version}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  )}
                  <Button variant="outline" size="icon" onClick={downloadFile}>
                    <Download className="h-4 w-4" />
                  </Button>
                </div>
              </CardHeader>
              <CardContent>
                <div className="space-y-4">
                  <div className="grid grid-cols-[180px,1fr] items-center gap-4 py-3 border-b">
                    <div className="text-sm font-medium text-muted-foreground">
                      Component ID
                    </div>
                    <div className="font-mono text-sm">
                      {component.versionedComponentId?.componentId}
                    </div>
                  </div>
                  <div className="grid grid-cols-[180px,1fr] items-center gap-4 py-3 border-b">
                    <div className="text-sm font-medium text-muted-foreground">
                      Version
                    </div>
                    <div className="font-mono text-sm">{versionChange}</div>
                  </div>
                  <div className="grid grid-cols-[180px,1fr] items-center gap-4 py-3 border-b">
                    <div className="text-sm font-medium text-muted-foreground">
                      Name
                    </div>
                    <div className="font-mono text-sm">
                      {component.componentName}
                    </div>
                  </div>
                  <div className="grid grid-cols-[180px,1fr] items-center gap-4 py-3 border-b">
                    <div className="text-sm font-medium text-muted-foreground">
                      Size
                    </div>
                    <div className="font-mono text-sm">
                      {Math.round((component?.componentSize || 0) / 1024)} KB
                    </div>
                  </div>
                  <div className="grid grid-cols-[180px,1fr] items-center gap-4 py-3">
                    <div className="text-sm font-medium text-muted-foreground">
                      Created At
                    </div>
                    <div className="font-mono text-sm">
                      {component.createdAt
                        ? formatRelativeTime(component.createdAt)
                        : "NA"}
                    </div>
                  </div>
                </div>
              </CardContent>
            </Card>
          </div>
        </div>
      </div>
    </ErrorBoundary>
  );
}
