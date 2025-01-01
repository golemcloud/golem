import { useEffect, useState } from "react";
import { Plus, Search, Layers } from "lucide-react";
import { useNavigate } from "react-router-dom";
import { Api } from "@/types/api";
import { SERVICE } from "@/service";

export const APIs = () => {
  const navigate = useNavigate();
  const [apis, setApis] = useState([] as Api[]);
  const [searchedApi, setSearchedApi] = useState([] as Api[]);

  useEffect(() => {
    SERVICE.getApiList().then((response) => {
      const newData = response.filter((api) => api.draft);
      setApis(newData);
      setSearchedApi(newData);
    });
  }, []);

  return (
    <div className="container mx-auto px-4 py-8">
      <div className="flex items-center justify-between gap-4 mb-8">
        <div className="relative flex-1">
          <Search className="absolute left-3 top-1/2 transform -translate-y-1/2 text-gray-400 h-5 w-5" />
          <input
            type="text"
            placeholder="Search APIs..."
            onChange={(e) =>
              setSearchedApi(
                apis.filter((api) => api.id.includes(e.target.value))
              )
            }
            className="w-full pl-10 pr-4 py-2 border border-gray-200 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500"
          />
        </div>
        <button
          onClick={() => navigate("/apis/create")}
          className="flex items-center gap-2 bg-blue-600 text-white px-4 py-2 rounded-lg hover:bg-blue-700"
        >
          <Plus className="h-5 w-5" />
          <span>New</span>
        </button>
      </div>

      {searchedApi.length > 0 ? (
        <div className="grid grid-cols-3 gap-6 overflow-scroll max-h-[75vh]">
          {searchedApi.map((api) => (
            <APICard
              key={api.id}
              name={api.id}
              version={api.version}
              routes={api.routes.length}
            />
          ))}
        </div>
      ) : (
        <div className="flex flex-col items-center justify-center py-12 border-2 border-dashed border-gray-200 rounded-lg">
          <Layers className="h-12 w-12 text-gray-400 mb-4" />
          <h3 className="text-lg font-medium mb-2">No APIs</h3>
          <p className="text-gray-500 mb-4">
            Create your first API to get started
          </p>
        </div>
      )}
    </div>
  );
};

interface APICardProps {
  name: string;
  version: string;
  routes: number;
}

const APICard = ({ name, version, routes }: APICardProps) => {
  const navigate = useNavigate();

  return (
    <div
      className="bg-white rounded-lg border border-gray-200 p-6 hover:shadow-md transition-shadow cursor-pointer"
      onClick={() => navigate(`/apis/${name}`)}
    >
      <div className="flex items-center justify-between mb-4">
        <h3 className="text-lg font-medium">{name}</h3>
        <div className="flex items-center gap-2">
          <span className="px-2 py-1 bg-gray-100 rounded text-sm">
            {version}
          </span>
        </div>
      </div>
      <div className="flex items-center text-sm text-gray-600">
        <span>Routes</span>
        <span className="ml-2">{routes}</span>
      </div>
    </div>
  );
};
