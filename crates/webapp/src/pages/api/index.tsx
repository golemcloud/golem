import { useEffect, useState } from "react";
import { Plus, Search, Layers } from "lucide-react";
import { useNavigate } from "react-router-dom";
import { Api } from "@/types/api";
import { API } from "@/service";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import ErrorBoundary from "@/components/errorBoundary";

export const APIs = () => {
  const navigate = useNavigate();
  const [apis, setApis] = useState([] as Api[]);
  const [searchedApi, setSearchedApi] = useState([] as Api[]);

  useEffect(() => {
    API.getApiList().then((response) => {
      const newData = response.filter((api) => api.draft);
      setApis(newData);
      setSearchedApi(newData);
    });
  }, []);

  return (
    <ErrorBoundary>
      <div className="container mx-auto px-4 py-8">
        <div className="flex items-center justify-between gap-4 mb-8">
          <div className="relative flex-1">
            <Search className="absolute left-3 top-1/2 transform -translate-y-1/2 text-muted-foreground h-5 w-5" />
            <Input
              type="text"
              placeholder="Search APIs..."
              onChange={(e) =>
                setSearchedApi(
                  apis.filter((api) => api.id.includes(e.target.value))
                )
              }
              className="pl-10"
            />
          </div>
          <Button onClick={() => navigate("/apis/create")} variant="default">
            <Plus className="h-5 w-5" />
            <span>New</span>
          </Button>
        </div>

        {searchedApi.length > 0 ? (
          <div className="grid grid-cols-3 gap-6 overflow-scroll max-h-[75vh]">
            {searchedApi.map((api) => (
              <APICard
                key={api.id}
                name={api.id}
                version={api.version}
                routes={api.routes.length}
              />
            ))}
          </div>
        ) : (
          <div className="flex flex-col items-center justify-center py-12 border-2 border-dashed border-muted rounded-lg">
            <Layers className="h-12 w-12 text-muted-foreground mb-4" />
            <h3 className="text-lg font-medium mb-2">No APIs</h3>
            <p className="text-muted-foreground mb-4">
              Create your first API to get started
            </p>
          </div>
        )}
      </div>
    </ErrorBoundary>
  );
};

interface APICardProps {
  name: string;
  version: string;
  routes: number;
}

const APICard = ({ name, version, routes }: APICardProps) => {
  const navigate = useNavigate();

  return (
    <Card
      onClick={() => navigate(`/apis/${name}`)}
      className="hover:shadow-md transition-shadow cursor-pointer"
    >
      <CardHeader>
        <CardTitle>{name}</CardTitle>
      </CardHeader>
      <CardContent>
        <div className="flex justify-between">
          <span>Routes</span>
          <span>{routes}</span>
        </div>
        <div className="mt-2">
          <span className="px-2 py-1 bg-muted rounded text-sm">{version}</span>
        </div>
      </CardContent>
    </Card>
  );
};
