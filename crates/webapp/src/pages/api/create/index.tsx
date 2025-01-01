import { useState } from "react";
import { useNavigate } from "react-router-dom";
import { PlusCircle, ArrowLeft } from "lucide-react";
import { Button } from "@/components/ui/button";
import { SERVICE } from "@/service";

const CreateAPI = () => {
  const navigate = useNavigate();
  const [apiName, setApiName] = useState("");
  const [version, setVersion] = useState("0.1.0");

  const onCreateApi = () => {
    SERVICE.createApi({
      id: apiName,
      version: version,
      routes: [],
      draft: true,
    }).then(() => navigate(`/apis/${apiName}`));
  };

  return (
    <div className="container mx-auto px-4 py-8 max-w-2xl">
      <div className="flex items-center gap-2 mb-2">
        <button
          onClick={() => navigate(-1)}
          className="text-xl  flex items-center text-gray-800 hover:text-gray-900"
        >
          <ArrowLeft className="h-7 w-7 mr-2" />
        </button>
        <h1 className="text-2xl font-semibold mb-2">Create a new API</h1>
      </div>
      <p className="text-gray-600 mb-8">
        Export worker functions as a REST API
      </p>

      <form className="space-y-6">
        <div className="grid  gap-4">
          <div className="col-span-2">
            <label className="block text-sm font-medium text-gray-700 mb-1">
              API Name
            </label>
            <div className="flex items-center">
              <input
                type="text"
                value={apiName}
                onChange={(e) => setApiName(e.target.value)}
                className="flex-1 border border-gray-200 rounded px-3 py-2 focus:outline-none focus:ring-2 focus:ring-blue-500"
                placeholder="Must be unique per project"
              />
            </div>
          </div>
        </div>

        <div>
          <label className="block text-sm font-medium text-gray-700 mb-1">
            Version
          </label>
          <input
            type="text"
            value={version}
            onChange={(e) => setVersion(e.target.value)}
            className="w-full border border-gray-200 rounded px-3 py-2 focus:outline-none focus:ring-2 focus:ring-blue-500"
            placeholder="Version prefix for your API"
          />
          <p className="mt-1 text-sm text-gray-500">
            Version prefix for your API
          </p>
        </div>

        <div className="flex justify-end">
          <Button
            type="submit"
            className="flex items-center space-x-2"
            onClick={onCreateApi}
            disabled={!apiName || !version}
          >
            <PlusCircle className="mr-2 size-4" />
            Create API
          </Button>
        </div>
      </form>
    </div>
  );
};

export default CreateAPI;
