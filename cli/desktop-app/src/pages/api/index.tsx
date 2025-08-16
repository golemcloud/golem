import { useEffect, useState } from "react";
import {
  GitBranch,
  Layers,
  Lock as LockIcon,
  Plus,
  Search,
} from "lucide-react";
import { useNavigate, useParams } from "react-router-dom";
import { HttpApiDefinition } from "@/types/golemManifest";
import { API } from "@/service";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import ErrorBoundary from "@/components/errorBoundary";
import { Badge } from "@/components/ui/badge.tsx";
import { removeDuplicateApis } from "@/lib/utils";

export const APIs = () => {
  const navigate = useNavigate();
  const [apis, setApis] = useState(
    [] as (HttpApiDefinition & { count?: number })[],
  );
  const [searchedApi, setSearchedApi] = useState(
    [] as (HttpApiDefinition & { count?: number })[],
  );
  const { appId } = useParams<{ appId: string }>();

  useEffect(() => {
    API.apiService.getApiList(appId!).then(response => {
      const newData = removeDuplicateApis(response);
      setApis(newData);
      setSearchedApi(newData);
    });
  }, []);

  return (
    <ErrorBoundary>
      <div className="container mx-auto px-6 py-10">
        <div className="flex items-center justify-between gap-4 mb-8">
          <div className="relative flex-1">
            <Search className="absolute left-3 top-1/2 transform -translate-y-1/2 text-muted-foreground h-5 w-5" />
            <Input
              type="text"
              placeholder="Search APIs..."
              onChange={e =>
                setSearchedApi(
                  apis.filter(api =>
                    api.id
                      ?.toLocaleLowerCase()
                      .includes(e.target.value.toLocaleLowerCase()),
                  ),
                )
              }
              className="pl-10 text-white"
            />
          </div>
          <Button
            onClick={() => navigate(`/app/${appId}/apis/create`)}
            variant="default"
          >
            <Plus className="h-5 w-5" />
            <span>New</span>
          </Button>
        </div>

        {searchedApi.length > 0 ? (
          <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-6 overflow-scroll max-h-[75vh]">
            {searchedApi.map(api => (
              <APICard
                key={api.id}
                name={api.id || ""}
                version={api.version}
                routes={api.routes?.length || 0}
                count={api.count || 0}
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
  count: number;
}

const APICard = ({ name, version, routes, count }: APICardProps) => {
  const navigate = useNavigate();
  const { appId } = useParams<{ appId: string }>();
  return (
    <Card
      className="from-background to-muted bg-gradient-to-br border-border w-full cursor-pointer hover:shadow-lg"
      onClick={() => navigate(`/app/${appId}/apis/${name}/version/${version}`)}
    >
      <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
        <CardTitle className="text-base font-semibold text-emerald-400">
          {name}
        </CardTitle>
        <Badge
          variant="outline"
          className="bg-emerald-500 text-white border-emerald-400 hover:bg-emerald-600"
        >
          {count || 0}
          <GitBranch className="ml-2 h-4 w-4" />
        </Badge>
      </CardHeader>
      <CardContent>
        <div className="flex flex-col flex-grow mt-2">
          <div className="flex items-center justify-between text-sm text-gray-300">
            <span>Latest Version</span>
            <span>Routes</span>
          </div>
          <div className="grid grid-cols-[auto,1fr,auto,auto] items-center gap-2 mt-2">
            <Badge
              variant="outline"
              className="bg-gray-600 text-white hover:bg-gray-500 transition-all duration-300"
            >
              {version}
            </Badge>
            <span className="w-4"></span>
            <LockIcon className="h-4 w-4 text-gray-400" />
            <div className="inline-flex items-center text-sm text-gray-300 w-3 justify-end">
              {routes}
            </div>
          </div>
        </div>
      </CardContent>
    </Card>
  );
};
