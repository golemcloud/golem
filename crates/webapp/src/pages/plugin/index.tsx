import {
  Component,
  ExternalLink,
  Globe,
  Plus,
  Search,
  LayoutGrid,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Badge } from "@/components/ui/badge";
import {
  Card,
  CardContent,
  CardDescription,
  CardFooter,
  CardHeader,
  CardTitle,
} from "@/components/ui/card.tsx";
import { useNavigate } from "react-router-dom";
import { useEffect, useState } from "react";
import { API } from "@/service";
import { Plugin } from "@/types";

export function PluginList() {
  const navigate = useNavigate();
  const [plugins, setPlugins] = useState<Plugin[]>([]);

  useEffect(() => {
    API.getPlugins().then((res) => {
      setPlugins(res);
    });
  }, []);

  return (
    <div className="p-4 min-h-screen bg-background text-foreground mx-auto max-w-7xl px-6 lg:px-8 py-4">
      <div className="flex gap-2 mb-4">
        <div className="relative flex-1">
          <Search className="absolute left-3 top-1/2 transform -translate-y-1/2 text-muted-foreground h-4 w-4" />
          <Input className="w-full pl-10" placeholder="Plugin name..." />
        </div>
        <Button variant="default" onClick={() => navigate("/plugins/create")}>
          <Plus className="h-4 w-4" />
          Create Plugin
        </Button>
      </div>
      {plugins.length > 0 ? (
        <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-6 overflow-scroll">
          {plugins.map((plugin) => (
            <Card
              key={plugin.name}
              className="flex flex-col h-full cursor-pointer"
              onClick={() =>
                navigate(`/plugins/${plugin.name}/${plugin.version}`)
              }
            >
              <CardHeader>
                <div className="flex justify-between items-start">
                  <CardTitle className="text-xl font-bold">
                    {plugin.name}
                  </CardTitle>
                  <Badge variant="secondary">{plugin.version}</Badge>
                </div>
                <CardDescription>{plugin.description}</CardDescription>
              </CardHeader>
              <CardContent className="flex-grow">
                <div className="space-y-2 flex items-center space-x-2">
                  <Badge variant="outline" className={"rounded-full"}>
                    {plugin.specs.type}
                  </Badge>
                  {plugin.specs.type === "OplogProcessor" && (
                    <Badge variant="outline" className={"rounded-full"}>
                      Component Version: {plugin.specs.componentVersion}
                    </Badge>
                  )}
                  <Badge variant="outline" className={"rounded-full"}>
                    {plugin.scope.type === "Global" ? (
                      <Globe className="w-4 h-4 mr-1" />
                    ) : (
                      <Component className="w-4 h-4 mr-1" />
                    )}
                    {plugin.scope.type}
                    (scope)
                  </Badge>
                </div>
              </CardContent>
              <CardFooter className="flex justify-between">
                {plugin.specs.validateUrl && (
                  <Button
                    variant="link"
                    size="sm"
                    onClick={() =>
                      window.open(plugin.specs.validateUrl, "_blank")
                    }
                  >
                    Validate <ExternalLink className="w-4 h-4" />
                  </Button>
                )}
                {plugin.specs.transformUrl && (
                  <Button
                    variant="link"
                    size="sm"
                    onClick={() =>
                      window.open(plugin.specs.transformUrl, "_blank")
                    }
                  >
                    Transform <ExternalLink className="w-4 h-4" />
                  </Button>
                )}
              </CardFooter>
            </Card>
          ))}
        </div>
      ) : (
        <div className="border-2 border-dashed border-gray-200 rounded-lg p-12 flex flex-col items-center justify-center">
          <div className="h-16 w-16 bg-gray-100 rounded-lg flex items-center justify-center mb-4">
            <LayoutGrid className="h-8 w-8 text-gray-400" />
          </div>
          <h2 className="text-xl font-semibold mb-2 text-center">No Plugin</h2>
          <p className="text-gray-500 mb-6 text-center">
            Create a new plugin to get started.
          </p>
        </div>
      )}
    </div>
  );
}
