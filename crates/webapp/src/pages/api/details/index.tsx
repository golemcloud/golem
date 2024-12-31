import { useParams, useNavigate } from "react-router-dom";
import { useEffect, useState } from "react";
import { Plus } from "lucide-react";
import APILeftNav from "./APILeftNav";
import { invoke } from "@tauri-apps/api/core";

const ApiMockData = [
  {
    createdAt: "2024-12-31T15:55:12.838362+00:00",
    draft: true,
    id: "great",
    routes: [],
    version: "0.2.0",
  },
  {
    createdAt: "2024-12-31T05:34:20.197542+00:00",
    draft: false,
    id: "vvvvv",
    routes: [],
    version: "0.1.0",
  },
];

const ApiDetailsMock = ApiMockData[0];

const APIDetails = () => {
  const { apiName } = useParams();
  const navigate = useNavigate();
  const [apiDetails, setApiDetails] = useState(ApiDetailsMock);

  useEffect(() => {
    const fetchData = async () => {
      //check the api https://release.api.golem.cloud/v1/api/definitions/305e832c-f7c1-4da6-babc-cb2422e0f5aa
      // eslint-disable-next-line @typescript-eslint/no-explicit-any, @typescript-eslint/no-unused-vars
      const response: any = await invoke("get_api");
      const apiData = ApiMockData.find((api) => api.id === apiName);
      if (apiData) {
        setApiDetails(apiData);
      } else {
        setApiDetails(ApiDetailsMock); // or handle the undefined case as needed
      }
    };
    fetchData().then((r) => r);
  }, []);

  return (
    <div className="flex">
      <APILeftNav />
      <div className="flex-1">
        <div className="flex items-center justify-between">
          <header className="w-full border-b bg-background py-2">
            <div className="mx-auto max-w-7xl px-6 lg:px-8">
              <div className="mx-auto max-w-2xl lg:max-w-none">
                <div className="flex items-center justify-between">
                  <div className="flex items-center gap-2">
                    <h1 className="line-clamp-1 font-medium leading-tight sm:leading-normal">
                      {apiName}
                    </h1>
                    <div className="flex items-center gap-1">
                      <div className="inline-flex items-center rounded-md px-2.5 py-0.5 text-xs font-semibold focus:outline-none bg-primary-background text-primary-soft  border border-primary-border w-fit font-mono">
                        {apiDetails?.version}
                      </div>
                    </div>
                  </div>
                </div>
              </div>
            </div>
          </header>
        </div>

        <div className="space-y-8 p-8">
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
