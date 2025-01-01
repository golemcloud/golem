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
import APILeftNav from "./apiLeftNav";
import { SERVICE } from "@/service";
import { Api } from "@/types/api";

export default function APINewVersion() {
  const navigate = useNavigate();
  const [version, setVersion] = useState("");
  const { apiName } = useParams();
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

  const onCreateApi = async () => {
    const payload = {
      ...activeApiDetails,
      version: version,
      createdAt: new Date().toISOString(),
    };
    SERVICE.postApi(payload).then(() => {
      navigate(`/apis/${apiName}`);
    });
  };

  return (
    <div className="flex">
      <APILeftNav />
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
        <div className="max-w-4xl mx-auto p-8">
          <label className="block text-sm font-medium text-gray-700 mb-1">
            New Version
          </label>
          <input
            type="text"
            value={version}
            onChange={(e) => setVersion(e.target.value)}
            className="w-full border border-gray-200 rounded px-3 py-2 focus:outline-none focus:ring-2 focus:ring-blue-500"
            placeholder="New Version prefix (0.0.0)"
          />
          <p className="mt-1 text-sm text-gray-500">
            Creating copy of version {activeApiDetails.version}
          </p>
          <div className="flex justify-end">
            <Button
              type="submit"
              className="flex items-center space-x-2"
              onClick={onCreateApi}
              disabled={!version}
            >
              <PlusCircle className="mr-2 size-4" />
              Create New Version
            </Button>
          </div>
        </div>
      </div>
    </div>
  );
}
