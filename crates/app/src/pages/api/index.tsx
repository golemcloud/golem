import { useEffect, useState } from "react";
import {
  GitBranch,
  Layers,
  Lock as LockIcon,
  Plus,
  Search,
} from "lucide-react";
import { useNavigate } from "react-router-dom";
import { Api } from "@/types/api";
import { API } from "@/service";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import ErrorBoundary from "@/components/errorBoundary";
import { Badge } from "@/components/ui/badge.tsx";
import { removeDuplicateApis } from "@/lib/utils";

export const APIs = () => {
  const navigate = useNavigate();
  const [apis, setApis] = useState([] as Api[]);
  const [searchedApi, setSearchedApi] = useState([] as Api[]);

  useEffect(() => {
    API.getApiList().then((response) => {
      const newData = removeDuplicateApis(response);
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
                  apis.filter((api) =>
                    api.id
                      .toLocaleLowerCase()
                      .includes(e.target.value.toLocaleLowerCase())
                  )
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

  return (
    <Card
      className="transition-all hover:shadow-lg hover:shadow-border/75 duration-150 w-full group cursor-pointer"
      onClick={() => navigate(`/apis/${name}/version/${version}`)}
    >
      <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
        <CardTitle className="text-sm font-medium">{name}</CardTitle>
        <Badge
          variant="outline"
          className="bg-primary-background text-primary-soft hover:bg-primary/50 active:bg-primary/50 border-primary-border"
        >
          {count || 0}
          <GitBranch className="ml-2 h-4 w-4" />
        </Badge>
      </CardHeader>
      <CardContent>
        <div className="flex flex-col flex-grow mt-2">
          <div className="flex items-center justify-between h-4 text-xs text-muted-foreground">
            <span>Latest Version</span>
            <span>Routes</span>
          </div>
          <div className="grid grid-cols-[1fr,auto,auto,auto] items-center gap-2 h-4 mt-2">
            <Badge
              variant="outline"
              className="w-fit font-mono font-light transition-all duration-300 group-hover:scale-110 group-hover:shadow-md"
            >
              {version}
            </Badge>
            <span className="w-4"></span>
            <LockIcon className="h-4 w-4 text-muted-foreground" />
            <div className="inline-flex items-center text-sm text-muted-foreground w-3 justify-end">
              {routes}
            </div>
          </div>
        </div>
      </CardContent>
    </Card>
  );
};
