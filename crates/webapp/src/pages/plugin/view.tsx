import { useNavigate, useParams } from "react-router-dom";
import { Plugin } from "@/types";
import { useEffect, useState } from "react";
import { API } from "@/service";
import { ArrowLeft, Component, Globe, Trash2 } from "lucide-react";
import {
  Card,
  CardContent,
  CardDescription,
  CardFooter,
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
import { Button } from "@/components/ui/button";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from "@/components/ui/alert-dialog";
import { Badge } from "@/components/ui/badge";
import { Separator } from "@/components/ui/separator";

export function PluginView() {
  const { pluginId, version } = useParams();
  const navigate = useNavigate();
  const [plugin, setPlugin] = useState<Plugin[]>([]);
  const [ver, setVer] = useState(version || "");
  const [currentVersion, setCurrentVersion] = useState<Plugin | null>(null);

  useEffect(() => {
    API.getPluginByName(pluginId!).then((res) => {
      setPlugin(res);
      const selectedVersion = version
        ? res.find((p) => p.version === version)
        : res[0];
      if (selectedVersion) {
        setCurrentVersion(selectedVersion);
        setVer(selectedVersion.version);
      }
    });
  }, [pluginId, version]);

  const handleVersionChange = (version: string) => {
    const selectedVersion = plugin.find((p) => p.version === version) || null;
    if (selectedVersion) {
      setCurrentVersion(selectedVersion);
      setVer(version);
      navigate(`/plugins/${pluginId}/${version}`);
    }
  };

  const handleDelete = () => {
    if (!currentVersion) return;
    API.deletePlugin(currentVersion.name, currentVersion.version)
      .then(() => {
        if (plugin.length > 1) {
          navigate(`/plugins/${plugin[0].name}`);
        } else {
          navigate("/plugins");
        }
      })
      .catch(console.error);
  };

  return (
    <div className="container mx-auto py-10 px-6">
      {currentVersion && (
        <Card className="w-full max-w-4xl mx-auto shadow-lg">
          <CardHeader className="p-4">
            <div className="flex justify-between items-center">
              <div className="flex items-center">
                <Button variant="link" onClick={() => navigate("/plugins")}>
                  <ArrowLeft className="w-5 h-5 mr-2" />
                </Button>
                <CardTitle className="text-2xl font-bold">
                  {currentVersion.name}
                </CardTitle>
              </div>
              <div className="flex items-center space-x-3">
                <Select onValueChange={handleVersionChange} value={ver}>
                  <SelectTrigger className="w-[180px]">
                    <SelectValue placeholder="Select version" />
                  </SelectTrigger>
                  <SelectContent>
                    {plugin.map((v) => (
                      <SelectItem key={v.version} value={v.version}>
                        {v.version}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
                <AlertDialog>
                  <AlertDialogTrigger asChild>
                    <Button variant="destructive" size="icon">
                      <Trash2 className="w-5 h-5" />
                    </Button>
                  </AlertDialogTrigger>
                  <AlertDialogContent>
                    <AlertDialogHeader>
                      <AlertDialogTitle>Confirm Deletion</AlertDialogTitle>
                      <AlertDialogDescription>
                        Are you sure you want to delete version {ver} of{" "}
                        {currentVersion.name}? This action cannot be undone.
                      </AlertDialogDescription>
                    </AlertDialogHeader>
                    <AlertDialogFooter>
                      <AlertDialogCancel>Cancel</AlertDialogCancel>
                      <AlertDialogAction onClick={handleDelete}>
                        Delete
                      </AlertDialogAction>
                    </AlertDialogFooter>
                  </AlertDialogContent>
                </AlertDialog>
              </div>
            </div>
            <CardDescription className="text-base text-gray-600 mt-2">
              {currentVersion.description}
            </CardDescription>
          </CardHeader>
          <Separator className="my-4" />
          <CardContent className="space-y-6">
            <div>
              <h3 className="font-semibold mb-2">Details</h3>
              <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
                {currentVersion.homepage && (
                  <div>
                    <h4 className="text-sm font-medium">Homepage:</h4>
                    <a
                      href={currentVersion.homepage}
                      className="text-blue-500 hover:underline"
                      target="_blank"
                      rel="noopener noreferrer"
                    >
                      {currentVersion.homepage}
                    </a>
                  </div>
                )}
              </div>
            </div>
            {currentVersion.specs?.type && (
              <div>
                <h3 className="font-semibold mb-2">Specs</h3>
                <div className="space-y-2">
                  <Badge variant="outline" className="mr-2">
                    {currentVersion.specs.type}
                  </Badge>
                  {currentVersion.specs.type === "OplogProcessor" && (
                    <>
                      <Badge variant="outline">
                        Component ID: {currentVersion.specs.componentId}
                      </Badge>
                      <Badge variant="outline">
                        Component Version:{" "}
                        {currentVersion.specs.componentVersion}
                      </Badge>
                    </>
                  )}
                  {currentVersion.specs.type === "ComponentTransformer" &&
                    currentVersion.specs.jsonSchema && (
                      <div>
                        <h4 className="text-sm font-medium mt-2">
                          JSON Schema:
                        </h4>
                        <pre className="bg-gray-100 p-2 rounded-md overflow-x-auto text-sm">
                          {currentVersion.specs.jsonSchema}
                        </pre>
                      </div>
                    )}
                </div>
              </div>
            )}
            {currentVersion.scope && (
              <div>
                <h3 className="font-semibold mb-2">Scope</h3>
                <Badge
                  variant="outline"
                  className="flex items-center text-sm w-fit"
                >
                  {currentVersion.scope.type === "Global" ? (
                    <Globe className="w-4 h-4 mr-2" />
                  ) : (
                    <Component className="w-4 h-4 mr-2" />
                  )}
                  {currentVersion.scope.type}
                </Badge>
                {currentVersion.scope.type === "Component" && (
                  <div className="mt-2">
                    <h4 className="text-sm font-medium">Component ID:</h4>
                    <p>{currentVersion.scope.componentID}</p>
                  </div>
                )}
              </div>
            )}
          </CardContent>
          <CardFooter className="flex justify-end space-x-4">
            {currentVersion.specs?.validateUrl && (
              <a
                href={currentVersion.specs.validateUrl}
                target="_blank"
                rel="noopener noreferrer"
                className="px-4 py-2 text-blue-500 hover:underline"
              >
                Validate
              </a>
            )}
            {currentVersion.specs?.transformUrl && (
              <a
                href={currentVersion.specs.transformUrl}
                target="_blank"
                rel="noopener noreferrer"
                className="px-4 py-2 bg-gray-500 text-white rounded-md shadow hover:bg-gray-600"
              >
                Transform
              </a>
            )}
          </CardFooter>
        </Card>
      )}
    </div>
  );
}
