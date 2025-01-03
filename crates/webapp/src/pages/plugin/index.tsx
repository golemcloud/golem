import { Component, Globe, Plus, Search, LayoutGrid } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Badge } from "@/components/ui/badge";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { useNavigate } from "react-router-dom";
import { useEffect, useState } from "react";
import { API } from "@/service";
import { Plugin } from "@/types";

export function PluginList() {
  const navigate = useNavigate();
  const [plugins, setPlugins] = useState<Plugin[]>([]);
  const [pluginsApi, setPluginsApi] = useState<Plugin[]>([]);

  useEffect(() => {
    const fetchPlugins = async () => {
      const res = await API.getPlugins();
      setPluginsApi(res);
      setPlugins(res);
    };
    fetchPlugins();
  }, []);

  const handleSearch = (e: React.ChangeEvent<HTMLInputElement>) => {
    const value = e.target.value.toLowerCase();
    setPlugins(
      pluginsApi.filter((plugin) => plugin.name.toLowerCase().includes(value))
    );
  };

  return (
    <div className="p-6 min-h-screen text-gray-900 mx-auto max-w-7xl">
      {/* Header Section */}
      <div className="flex justify-between items-center mb-6">
        <div className="relative w-full">
          <Search className="absolute left-3 top-1/2 transform -translate-y-1/2 text-gray-400 h-5 w-5" />
          <Input
            className="w-full pl-10 border-gray-300 rounded-md"
            placeholder="Search plugins..."
            onChange={handleSearch}
          />
        </div>
        <Button
          variant="default"
          className="ml-4"
          onClick={() => navigate("/plugins/create")}
        >
          <Plus className="mr-2 h-5 w-5" />
          Create Plugin
        </Button>
      </div>

      {/* Plugin List */}
      {plugins.length > 0 ? (
        <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-6">
          {plugins.map((plugin) => (
            <Card
              key={plugin.name}
              className="flex flex-col h-full cursor-pointer hover:shadow-lg transition-shadow"
              onClick={() =>
                navigate(`/plugins/${plugin.name}/${plugin.version}`)
              }
            >
              <CardHeader>
                <div className="flex justify-between items-start">
                  <CardTitle className="text-lg font-bold">
                    {plugin.name}
                  </CardTitle>
                  <Badge variant="secondary" className="text-sm">
                    {plugin.version}
                  </Badge>
                </div>
                <CardDescription className="text-sm text-gray-500">
                  {plugin.description}
                </CardDescription>
              </CardHeader>
              <CardContent className="flex-grow">
                <div className="flex flex-wrap gap-2">
                  <Badge variant="outline" className="rounded-full text-sm">
                    {plugin.specs.type}
                  </Badge>
                  {plugin.specs.type === "OplogProcessor" && (
                    <Badge variant="outline" className="rounded-full text-sm">
                      Component Version: {plugin.specs.componentVersion}
                    </Badge>
                  )}
                  <Badge
                    variant="outline"
                    className="flex items-center rounded-full text-sm"
                  >
                    {plugin.scope.type === "Global" ? (
                      <Globe className="w-4 h-4 mr-1" />
                    ) : (
                      <Component className="w-4 h-4 mr-1" />
                    )}
                    {plugin.scope.type} (scope)
                  </Badge>
                </div>
              </CardContent>
            </Card>
          ))}
        </div>
      ) : (
        <div className="flex flex-col items-center justify-center h-64 border-2 border-dashed border-gray-300 rounded-lg">
          <LayoutGrid className="h-12 w-12 text-gray-400 mb-4" />
          <h2 className="text-lg font-medium text-gray-700">
            No Plugins Found
          </h2>
          <p className="text-sm text-gray-500">
            Create a new plugin to get started.
          </p>
        </div>
      )}
    </div>
  );
}
