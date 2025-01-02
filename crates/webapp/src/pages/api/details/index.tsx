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
import { SERVICE } from "@/service";
import { Api } from "@/types/api";

const APIDetails = () => {
  const { apiName } = useParams();
  const navigate = useNavigate();
  const [apiDetails, setApiDetails] = useState([] as Api[]);
  const [activeApiDetails, setActiveApiDetails] = useState({} as Api);

  useEffect(() => {
    if (apiName) {
      SERVICE.getApi(apiName).then((response) => {
        setApiDetails(response);
        setActiveApiDetails(response[response.length - 1]);
      });
    }
  }, [apiName]);

  return (
    <div className="flex">
      <ApiLeftNav />
      <div className="flex-1">
        <div className="flex items-center justify-between">
          <header className="w-full border-b bg-background py-2">
            <div className="max-w-7xl px-6 lg:px-8">
              <div className="mx-auto max-w-2xl lg:max-w-none">
                <div className="flex items-center justify-between">
                  <div className="flex items-center gap-2">
                    <h1 className="line-clamp-1 font-medium leading-tight sm:leading-normal">
                      {apiName}
                    </h1>
                    <div className="flex items-center gap-1">
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
                          <SelectTrigger className="w-20 h-6">
                            <SelectValue>
                              {activeApiDetails.version}
                            </SelectValue>
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
              </div>
            </div>
          </header>
        </div>

        <div className="overflow-scroll h-[85vh] space-y-8 p-8">
          <section>
            <div className="flex items-center justify-between mb-4">
              <h2 className="text-xl font-semibold">Routes</h2>
              <button
                className="flex items-center gap-2 text-blue-600 hover:text-blue-700"
                onClick={() => navigate(`/apis/${apiName}/routes/new`)}
              >
                <Plus className="h-5 w-5" />
                <span>Add</span>
              </button>
            </div>
            <div className="bg-white rounded-lg border border-gray-200 p-8 text-center text-gray-500">
              No routes defined for this API version.
            </div>
          </section>
        </div>
      </div>
    </div>
  );
};

export default APIDetails;
