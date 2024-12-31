/* eslint-disable @typescript-eslint/no-explicit-any */
import { useEffect, useState } from "react";
import { Layers, PlusCircle } from "lucide-react";
import { useNavigate } from "react-router-dom";
import { Button } from "@/components/ui/button.tsx";
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

const APISection = () => {
  const navigate = useNavigate();
  const [apis, setApis] = useState(ApiMockData);
  useEffect(() => {
    const fetchData = async () => {
      //check the api https://release.api.golem.cloud/v1/api/definitions/305e832c-f7c1-4da6-babc-cb2422e0f5aa
      const response: any = await invoke("get_api");
      setApis(response);
    };
    fetchData().then((r) => r);
  }, []);

  return (
    <div className="bg-white rounded-lg border border-gray-200 p-6">
      <div className="flex justify-between items-center mb-6">
        <h2 className="text-xl font-semibold">APIs</h2>
        <button
          onClick={() => {
            navigate("/apis");
          }}
        >
          View All
        </button>
      </div>
      {apis.length > 0 ? (
        <div className="grid gap-4">
          {apis.map((api) => (
            <button
              key={api.id}
              className="flex w-full items-center justify-between py-2 px-4 hover:bg-gray-50 rounded border border-gray-200"
              onClick={() => {
                navigate(`/apis/${api.id}`);
              }}
            >
              <span className="text-gray-700">{api.id}</span>
              <span className="text-gray-500 text-sm">{api.version}</span>
            </button>
          ))}
        </div>
      ) : (
        <div className="flex flex-col items-center justify-center py-12 border-2 border-dashed border-gray-200 rounded-lg">
          <Layers className="h-12 w-12 text-gray-400 mb-4" />
          <h3 className="text-lg font-medium mb-2">No APIs</h3>
          <p className="text-gray-500 mb-4">
            Create your first API to get started
          </p>
          <Button
            onClick={() => {
              navigate("/apis/create");
            }}
          >
            <PlusCircle className="mr-2 size-4" />
            Create API
          </Button>
        </div>
      )}
    </div>
  );
};

export default APISection;
