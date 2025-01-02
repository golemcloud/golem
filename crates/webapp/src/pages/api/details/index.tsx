import { useParams, useNavigate } from "react-router-dom";
import { useEffect, useState } from "react";
import { Plus } from "lucide-react";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import ApiLeftNav from "./apiLeftNav.tsx";
import { API } from "@/service";
import { Api } from "@/types/api";
import { Button } from "@/components/ui/button.tsx";
import ErrorBoundary from "@/components/errorBoundary.tsx";

const APIDetails = () => {
  const { apiName } = useParams();
  const navigate = useNavigate();
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

  return (
    <ErrorBoundary>
      <div className="flex">
        <ApiLeftNav />
        <div className="flex-1 flex flex-col">
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

          <main className="flex-1 overflow-y-auto p-6">
            <section>
              <div className="flex items-center justify-between mb-4">
                <h2 className="text-lg font-medium text-foreground">Routes</h2>
                <Button
                  variant="outline"
                  onClick={() => navigate(`/apis/${apiName}/routes/new`)}
                  className="flex items-center gap-2"
                >
                  <Plus className="h-5 w-5" />
                  <span>Add</span>
                </Button>
              </div>
              <div className="bg-muted rounded-lg border border-muted-foreground p-6 text-center text-muted-foreground">
                No routes defined for this API version.
              </div>
            </section>
          </main>
        </div>
      </div>
    </ErrorBoundary>
  );
};

export default APIDetails;
