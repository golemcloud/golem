import ErrorBoundary from "@/components/errorBoundary";
import { Badge } from "@/components/ui/badge.tsx";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { removeDuplicateApis } from "@/lib/utils";
import { API } from "@/service";
import { HttpApiDefinition } from "@/types/golemManifest";
import { ArrowRight, Layers, PlusCircle, Server } from "lucide-react";
import { useEffect, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";

export function APISection() {
  const navigate = useNavigate();
  const { appId } = useParams<{ appId: string }>();
  const [apis, setApis] = useState(
    [] as (HttpApiDefinition & { count?: number })[],
  );

  useEffect(() => {
    API.apiService.getApiList(appId!).then(response => {
      const newData = removeDuplicateApis(response);
      setApis(newData);
    });
  }, []);

  return (
    <ErrorBoundary>
      <Card>
        <CardHeader className="flex flex-row items-center justify-between">
          <CardTitle className="text-xl font-semibold flex items-center gap-2 text-primary">
            <Server className="w-5 h-5 text-muted-foreground" />
            APIs
          </CardTitle>
          <Button
            variant="ghost"
            onClick={() => navigate(`/app/${appId}/apis`)}
          >
            View All
            <ArrowRight className="w-4 h-4 ml-1" />
          </Button>
        </CardHeader>
        <CardContent className="space-y-2">
          {apis && apis.length > 0 ? (
            apis.map(api => (
              <div
                key={api.id}
                className="flex items-center justify-between border rounded-lg p-3 hover:bg-muted/50 cursor-pointer bg-gradient-to-br from-background to-muted hover:shadow-lg transition-all"
                onClick={() => {
                  navigate(
                    `/app/${appId}/apis/${api.id}/version/${api.version}`,
                  );
                }}
              >
                <p className="text-sm font-medium">{api.id}</p>
                <Badge variant="secondary">{api.version}</Badge>
              </div>
            ))
          ) : (
            <div className="border-2 border-dashed border-gray-200 rounded-lg p-12 flex flex-col items-center justify-center">
              <div className="h-16 w-16 bg-gray-100 rounded-lg flex items-center justify-center mb-4">
                <Layers className="h-8 w-8 text-gray-400" />
              </div>
              <h2 className="text-xl font-semibold mb-2 text-center">
                No APIs
              </h2>
              <p className="text-gray-500 mb-6 text-center">
                Create your first API to get started.
              </p>
              <Button onClick={() => navigate(`/app/${appId}/apis/create`)}>
                <PlusCircle className="mr-2 size-4" />
                Create API
              </Button>
            </div>
          )}
        </CardContent>
      </Card>
    </ErrorBoundary>
  );
}
