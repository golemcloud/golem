import { Layers, PlusCircle } from "lucide-react";
import { useNavigate } from "react-router-dom";
import { Button } from "@/components/ui/button.tsx";
import { useEffect, useState } from "react";
import { formatRelativeTime } from "@/lib/utils";
import { API } from "@/service";
import { Component } from "@/types/component.ts";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card.tsx";
import { Badge } from "@/components/ui/badge.tsx";
import ErrorBoundary from "@/components/errorBoundary";

export const ComponentsSection = () => {
  const navigate = useNavigate();
  const [components, setComponents] = useState<{ [key: string]: Component }>(
    {}
  );
  useEffect(() => {
    API.getComponentByIdAsKey().then((response) => setComponents(response));
  }, []);

  return (
    <ErrorBoundary>
      <Card className={"rounded-lg lg:col-span-2"}>
        <CardHeader>
          <div className="flex justify-between items-center mb-6">
            <CardTitle>Components</CardTitle>
            <Button variant="outline" onClick={() => navigate("/components")}>
              View All
            </Button>
          </div>
        </CardHeader>
        <CardContent>
          {Object.keys(components).length > 0 ? (
            <div className="p-4 pt-0 md:p-6 md:pt-0 flex-1 w-full">
              <div className="grid w-full grid-cols-1 gap-4 md:gap-6 md:grid-cols-2">
                {Object.values(components).map((data: Component) => (
                  <Card
                    className={
                      "rounded-lg cursor-pointer text-card-foreground shadow transition-all hover:shadow-lg hover:shadow-border/75 duration-150"
                    }
                    onClick={() => navigate(`/components/${data.componentId}`)}
                  >
                    <CardHeader className="pb-2">
                      <div className="flex justify-between items-center">
                        <CardTitle className="font-medium">
                          {data.componentName}
                        </CardTitle>
                        <Badge variant="outline">v{data.versionId?.[0]}</Badge>
                      </div>
                      <CardDescription
                        className={"text-xs font-light text-muted-foreground"}
                      >
                        {data.createdAt
                          ? formatRelativeTime(data.createdAt)
                          : "NA"}
                      </CardDescription>
                    </CardHeader>
                    <CardContent>
                      <div className="mt-2 flex w-full items-center gap-2">
                        <Badge
                          variant="outline"
                          className="font-mono font-extralight transition-colors focus:outline-none focus:ring-2 focus:ring-ring focus:ring-offset-2 hover:bg-accent hover:text-accent-foreground active:bg-accent/50 active:text-accent-foreground text-muted-foreground"
                        >
                          {data?.exports?.[0]?.functions?.length} Exports
                        </Badge>
                        <Badge
                          variant="outline"
                          className="font-mono font-extralight transition-colors focus:outline-none focus:ring-2 focus:ring-ring focus:ring-offset-2 hover:bg-accent hover:text-accent-foreground active:bg-accent/50 active:text-accent-foreground text-muted-foreground"
                        >
                          {Math.round((data?.componentSize || 0) / 1024)} KB
                        </Badge>
                        <Badge
                          variant="outline"
                          className="font-mono font-extralight transition-colors focus:outline-none focus:ring-2 focus:ring-ring focus:ring-offset-2 hover:bg-accent hover:text-accent-foreground active:bg-accent/50 active:text-accent-foreground text-muted-foreground"
                        >
                          {data.componentType}
                        </Badge>
                      </div>
                    </CardContent>
                  </Card>
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
        </CardContent>
      </Card>
    </ErrorBoundary>
  );
};
