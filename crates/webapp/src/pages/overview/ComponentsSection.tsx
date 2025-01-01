import { Layers, PlusCircle } from "lucide-react";
import { useNavigate } from "react-router-dom";
import { Button } from "@/components/ui/button.tsx";
import {useEffect, useState} from "react";
import { formatRelativeTime } from "@/lib/utils";
import {SERVICE} from "@/service";


const ComponentsSection = () => {
  const navigate = useNavigate();
  const [components, setComponents] = useState({} as any);

  useEffect(() => {
    SERVICE.getComponents().then((response) => {
        const componentData = {} as any;
        response.forEach((data: any) => {
            componentData[data.versionedComponentId.componentId] = {
                componentName: data.componentName,
                componentId: data.versionedComponentId.componentId,
                createdAt: data.createdAt,
                exports: data.metadata.exports,
                componentSize: data.componentSize,
                componentType: data.componentType,
                versionId: [
                    ...(componentData[data.versionedComponentId.componentId]
                        ?.versionId || []),
                    data.versionedComponentId.version,
                ],
            };
        });
        setComponents(componentData)
    });
  }, [SERVICE]);
  return (
    <div className="rounded-lg border p-6 overflow-scroll max-h-[50vh] min-h-[50vh]">
      <div className="flex justify-between items-center mb-6">
        <h2 className="text-xl font-semibold">Components</h2>
        <Button variant="link"
          onClick={() => {
            navigate("/components");
          }}
        >
          View All
        </Button>
      </div>
      {Object.keys(components).length > 0 ? (
        <div className="p-4 pt-0 md:p-6 md:pt-0 flex-1 w-full">
          <div className="grid w-full grid-cols-1 gap-4 md:gap-6 md:grid-cols-2">
            {Object.values(components).map((data: any) => (
              <div
                key={data.componentId}
                className="rounded-lg border bg-card text-card-foreground shadow transition-all hover:shadow-lg hover:shadow-border/75 duration-150 h-full flex-col gap-2 p-4"
                onClick={() => navigate(`/components/${data.componentId}`)}
              >
                <div className="flex h-12 flex-row items-start justify-between pb-2 text-base">
                  <div className="flex flex-col items-start">
                    <h3 className="font-medium">{data.componentName}</h3>
                    <span className="text-xs font-light text-muted-foreground">
                      {formatRelativeTime(data.createdAt)}
                    </span>
                  </div>
                  <div className="inline-flex items-center rounded-md px-2.5 py-0.5 text-xs transition-colors focus:outline-none focus:ring-2 focus:ring-ring focus:ring-offset-2 bg-primary-background text-primary-soft hover:bg-primary/50 active:bg-primary/50 border border-primary-border font-mono font-normal">
                    v{data.versionId?.[0]}
                  </div>
                </div>
                <div className="mt-2 flex w-full items-center gap-2">
                  <div className="rounded-md border px-2.5 py-0.5 transition-colors focus:outline-none focus:ring-2 focus:ring-ring focus:ring-offset-2 hover:bg-accent hover:text-accent-foreground active:bg-accent/50 active:text-accent-foreground flex h-5 items-center gap-2 text-xs font-normal text-muted-foreground">
                    <span>{data.exports[0].functions.length}</span>
                    <span>Exports</span>
                  </div>
                  <div className="rounded-md border px-2.5 py-0.5 transition-colors focus:outline-none focus:ring-2 focus:ring-ring focus:ring-offset-2 hover:bg-accent hover:text-accent-foreground active:bg-accent/50 active:text-accent-foreground flex h-5 items-center gap-2 text-xs font-normal text-muted-foreground">
                    <span>{Math.round(data.componentSize / 1024)} KB</span>
                  </div>
                  <div className="rounded-md border px-2.5 py-0.5 transition-colors focus:outline-none focus:ring-2 focus:ring-ring focus:ring-offset-2 hover:bg-accent hover:text-accent-foreground active:bg-accent/50 active:text-accent-foreground flex h-5 items-center gap-2 text-xs font-normal text-muted-foreground">
                    <span>{data.componentType}</span>
                  </div>
                </div>
              </div>
            ))}
          </div>
        </div>
      ) : (
        <div className="flex flex-col items-center justify-center py-8 border-2 border-dashed rounded-lg">
          <Layers className="h-12 w-12 mb-4" />
          <h3 className="text-lg font-medium mb-2">No Components</h3>
          <p className="text-gray-500 mb-4">
            Create your first component to get started
          </p>
          <Button
            onClick={() => {
              navigate("/components/create");
            }}
          >
            <PlusCircle className="mr-2 size-4" />
            Create Component
          </Button>
        </div>
      )}
    </div>
  );
};

export default ComponentsSection;
