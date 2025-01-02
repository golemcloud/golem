import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { useNavigate } from "react-router-dom";
import { Layers, PlusCircle } from "lucide-react";
import { useEffect, useState } from "react";
import { Api } from "@/types/api.ts";
import { API } from "@/service";
import ErrorBoundary from "@/components/errorBoundary";

export function APISection() {
  const navigate = useNavigate();
  const [apis, setApis] = useState([] as Api[]);

  useEffect(() => {
    API.getApiList().then((response) => {
      setApis(response.filter((api) => api.draft));
    });
  }, []);

  return (
    <ErrorBoundary>
      <Card className={"rounded-lg"}>
        <CardHeader>
          <div className="flex justify-between items-center mb-6">
            <CardTitle>APIs</CardTitle>
            <Button variant="outline" onClick={() => navigate("/apis")}>
              View All
            </Button>
          </div>
        </CardHeader>
        <CardContent>
          {apis.length > 0 ? (
            <div className="grid gap-4">
              {apis.map((api) => (
                <button
                  key={api.id}
                  className="flex w-full items-center justify-between py-2 px-4 hover:bg-accent rounded border border-gray"
                  onClick={() => {
                    navigate(`/apis/${api.id}`);
                  }}
                >
                  <span className="text-gray-500">{api.id}</span>
                  <span className="text-gray-500 text-sm">{api.version}</span>
                </button>
              ))}
            </div>
          ) : (
            <div className="rounded-lg border-2 border-dashed border-border p-12 text-center grid place-items-center h-full w-full">
              <Layers className="h-12 w-12 text-gray-400 mb-4" />
              <h3 className="text-lg font-medium mb-2">No APIs</h3>
              <p className="text-gray-500 mb-4">
                Create your first API to get started
              </p>
              <Button onClick={() => navigate("/apis/create")}>
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
