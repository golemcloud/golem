import { useState, useEffect } from "react";
import { useParams, useNavigate } from "react-router-dom";
import { Button } from "@/components/ui/button";
import { PlusCircle } from "lucide-react";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import ApiLeftNav from "./apiLeftNav.tsx";
import { API } from "@/service";
import { Api } from "@/types/api";
import ErrorBoundary from "@/components/errorBoundary";

export default function APINewVersion() {
  const navigate = useNavigate();
  const [version, setVersion] = useState("");
  const { apiName } = useParams();
  const [error, setError] = useState(false);
  const [apiDetails, setApiDetails] = useState([] as Api[]);
  const [activeApiDetails, setActiveApiDetails] = useState({} as Api);

  useEffect(() => {
    if (apiName) {
      API.getApi(apiName).then((response) => {
        setApiDetails(response);
        setActiveApiDetails(response[response.length - 1]);
      });
    }
  }, [apiName]);

  const onCreateApi = async () => {
    if (!version) {
      setError(true);
      return;
    }
    const payload = {
      ...activeApiDetails,
      version: version,
      createdAt: new Date().toISOString(),
    };
    API.postApi(payload).then(() => {
      navigate(`/apis/${apiName}`);
    });
  };

  return (
    <ErrorBoundary>
      <div className="flex bg-background text-foreground">
        <ApiLeftNav />
        <div className="flex-1">
          <header className="w-full border-b bg-background py-4">
            <div className="mx-auto px-6 lg:px-8">
              <div className="flex items-center gap-4">
                <h1 className="text-xl font-semibold text-foreground truncate">
                  {apiName}
                </h1>
                <div className="flex items-center gap-2">
                  {activeApiDetails.version && (
                    <Select
                      defaultValue={activeApiDetails.version}
                      onValueChange={(version) => {
                        const selectedApi = apiDetails.find(
                          (api) => api.version === version
                        );
                        if (selectedApi) {
                          setActiveApiDetails(selectedApi);
                        }
                      }}
                    >
                      <SelectTrigger className="w-28">
                        <SelectValue>{activeApiDetails.version}</SelectValue>
                      </SelectTrigger>
                      <SelectContent>
                        {apiDetails.map((api) => (
                          <SelectItem value={api.version} key={api.version}>
                            {api.version}{" "}
                            {api.draft ? "(Draft)" : "(Published)"}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  )}
                </div>
              </div>
            </div>
          </header>

          <main className="max-w-4xl mx-auto p-14">
            <div className="space-y-4">
              <Label htmlFor="version" className="text-sm font-medium">
                New Version
              </Label>
              <Input
                id="version"
                type="text"
                value={version}
                onChange={(e) => {
                  setError(false);
                  setVersion(e.target.value);
                }}
                placeholder="New Version prefix (0.1.0)"
                className={`w-full ${error ? "border-destructive" : ""}`}
              />
              {error && (
                <p className="text-sm text-destructive mt-1">
                  Version is required.
                </p>
              )}
              <p className="text-sm text-muted-foreground">
                Creating a copy of version {activeApiDetails.version}
              </p>
              <div className="flex justify-end">
                <Button type="submit" onClick={onCreateApi}>
                  <PlusCircle className="mr-2 h-5 w-5" />
                  Create New Version
                </Button>
              </div>
            </div>
          </main>
        </div>
      </div>
    </ErrorBoundary>
  );
}
