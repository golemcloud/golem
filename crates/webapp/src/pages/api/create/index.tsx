import { useState } from "react";
import { useNavigate } from "react-router-dom";
import { PlusCircle, ArrowLeft } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { API } from "@/service";
import ErrorBoundary from "@/components/errorBoundary";

const CreateAPI = () => {
  const navigate = useNavigate();
  const [apiName, setApiName] = useState("");
  const [version, setVersion] = useState("0.1.0");
  const [errors, setErrors] = useState({ apiName: "", version: "" });

  const validateForm = () => {
    const newErrors = { apiName: "", version: "" };
    if (!apiName.trim()) newErrors.apiName = "API Name is required.";
    if (!version.trim()) newErrors.version = "Version is required.";
    setErrors(newErrors);
    return !newErrors.apiName && !newErrors.version;
  };

  const onCreateApi = (e: React.FormEvent) => {
    e.preventDefault();
    if (validateForm()) {
      API.createApi({
        id: apiName,
        version: version,
        routes: [],
        draft: true,
      }).then(() => navigate(`/apis/${apiName}`));
    }
  };

  return (
    <ErrorBoundary>
      <div className="container mx-auto px-4 py-16 max-w-2xl">
        <h1 className="text-2xl font-semibold mb-2">Create a new API</h1>
        <p className="text-muted-foreground mb-8">
          Export worker functions as a REST API
        </p>

        <form className="space-y-6" onSubmit={onCreateApi}>
          <div className="grid gap-4">
            <div>
              <Label htmlFor="apiName" className="mb-1">
                API Name
              </Label>
              <Input
                id="apiName"
                type="text"
                value={apiName}
                onChange={(e) => {
                  setErrors({ ...errors, apiName: "" });
                  setApiName(e.target.value);
                }}
                placeholder="Must be unique per project"
                className={`${errors.apiName ? "border-destructive" : ""}`}
              />
              {errors.apiName && (
                <p className="text-sm text-destructive mt-1">
                  {errors.apiName}
                </p>
              )}
            </div>
          </div>

          <div>
            <Label htmlFor="version" className="mb-1">
              Version
            </Label>
            <Input
              id="version"
              type="text"
              value={version}
              onChange={(e) => {
                setErrors({ ...errors, version: "" });
                setVersion(e.target.value);
              }}
              placeholder="Version prefix for your API"
              className={`${errors.version ? "border-destructive" : ""}`}
            />
            {errors.version && (
              <p className="text-sm text-destructive mt-1">{errors.version}</p>
            )}
            <p className="mt-1 text-sm text-muted-foreground">
              Version prefix for your API
            </p>
          </div>

          <div className="flex justify-between">
            <Button
              type="button"
              variant="secondary"
              onClick={() => navigate(-1)}
            >
              <ArrowLeft className="mr-2 h-5 w-5" />
              Back
            </Button>
            <Button
              type="submit"
              variant="default"
              className="flex items-center space-x-2"
            >
              <PlusCircle className="mr-2 h-5 w-5" />
              Create API
            </Button>
          </div>
        </form>
      </div>
    </ErrorBoundary>
  );
};

export default CreateAPI;
