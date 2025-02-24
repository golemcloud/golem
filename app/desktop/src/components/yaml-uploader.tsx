/* eslint-disable @typescript-eslint/ban-ts-comment */
/* eslint-disable @typescript-eslint/no-explicit-any */
// @ts-nocheck
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input.tsx";
import * as yaml from "js-yaml";
import { Upload } from "lucide-react";
import { useEffect, useState } from "react";
import { YamlEditor } from "./yaml-editor";
import { API } from "@/service";
import { Api } from "@/types/api.ts";
import { useNavigate, useParams, useSearchParams } from "react-router-dom";
import { ENDPOINT } from "@/service/endpoints.ts";

export default function YamlUploader() {
  const { apiName, version } = useParams();
  const navigate = useNavigate();
  const [queryParams] = useSearchParams();
  const path = queryParams.get("path");
  const method = queryParams.get("method");
  const reload = queryParams.get("reload");
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [yamlContent, setYamlContent] = useState<string>("");
  const [fileName, setFileName] = useState<string>("");
  const [isOpen, setIsOpen] = useState(false);
  const [isSubmitting, setIsSubmitting] = useState(false);
  const [activeApiDetails, setActiveApiDetails] = useState<Api | null>(null);

  useEffect(() => {
    const fetchData = async () => {
      if (!apiName) return;
      try {
        setIsLoading(true);
        const [apiResponse, componentResponse] = await Promise.all([
          API.getApi(apiName),
          API.getComponentByIdAsKey(),
        ]);
        const selectedApi = apiResponse.find(api => api.version === version);
        setActiveApiDetails(selectedApi!);
      } catch (error) {
        console.error("Failed to fetch data:", error);
        setFetchError("Failed to load required data. Please try again.");
      } finally {
        setIsLoading(false);
      }
    };

    fetchData();
  }, [apiName, version, path, method]);

  const handleFileChange = async (e: React.ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0];
    if (!file) return;

    try {
      setFileName(file.name);
      const content = await file.text();

      // Validate YAML before setting content
      yaml.load(content);
      setYamlContent(content);
    } catch (error) {
      setError("Invalid YAML file.");
      // You might want to show an error toast or message here
    }
  };

  const onSubmit = async (payload: any) => {
    try {
      setIsSubmitting(true);

      const apiResponse = await API.getApi(apiName!);
      const selectedApi = apiResponse.find(api => api.version === version);
      const r = await API.callApi(
        ENDPOINT.putApi(apiName, version),
        "PUT",
        payload,
        { "Content-Type": "application/yaml" },
      ).then(() => {
        navigate(`/apis/${apiName}/version/${version}?reload=${!reload}`);
      });
    } catch (error) {
      console.error("Failed to create route:", error);
    } finally {
      setIsSubmitting(false);
    }
  };

  function validateYamlContent(yamlString: string): {
    isValid: boolean;
    errors: ValidationError[];
  } {
    let parsedData: any;
    const errors: ValidationError[] = [];

    // Step 1: Parse YAML to JSON
    try {
      parsedData = yaml.load(yamlString);
    } catch (error) {
      return {
        isValid: false,
        errors: ["Invalid YAML format."],
      };
    }

    // Step 2: Validate main API structure
    if (!parsedData.id || typeof parsedData.id !== "string") {
      errors.push("Invalid or missing 'id' field.");
    }

    if (!parsedData.version || typeof parsedData.version !== "string") {
      errors.push("Invalid or missing 'version' field.");
    }

    if (typeof parsedData.draft !== "boolean") {
      errors.push("Invalid or missing 'draft' field.");
    }

    if (!Array.isArray(parsedData.routes)) {
      errors.push("Invalid or missing 'routes' array.");
    } else {
      // Step 3: Validate each route
      parsedData.routes.forEach((route: any, index: number) => {
        const routePath = `routes[${index}]`;

        if (
          !route.method ||
          ![
            "Get",
            "Post",
            "Put",
            "Delete",
            "Patch",
            "Head",
            "Options",
            "Trace",
            "Connect",
          ].includes(route.method)
        ) {
          errors.push("Invalid HTTP method.");
        }

        if (!route.path || typeof route.path !== "string") {
          errors.push("Invalid or missing 'path' field.");
        }

        if (!route.binding || typeof route.binding !== "object") {
          errors.push("Invalid or missing 'binding' object.");
        } else {
          const { bindingType, componentId, response } = route.binding;

          if (
            !["default", "file-server", "cors-preflight"].includes(bindingType)
          ) {
            errors.push("Invalid 'bindingType'.");
          }

          if (bindingType === "cors-preflight") {
            if (!componentId || typeof componentId !== "object") {
              errors.push(
                "Missing 'componentId' for 'cors-preflight' binding.",
              );
            } else {
              if (
                !componentId.componentId ||
                typeof componentId.componentId !== "string"
              ) {
                errors.push("Invalid 'componentId'.");
              }

              if (typeof componentId.version !== "number") {
                errors.push("Invalid 'componentId.version'.");
              }
            }

            if (!response || typeof response !== "string") {
              errors.push("Missing 'response' for 'cors-preflight' binding.");
            }
          }
        }
      });
    }

    return {
      isValid: errors.length === 0,
      errors,
    };
  }

  const handleSubmit = async () => {
    try {
      const { isValid, errors } = validateYamlContent(yamlContent);
      if (!isValid) {
        setError(errors.join("\n"));
        return;
      }
      setIsSubmitting(true);

      onSubmit(yamlContent);

      // Reset state and close dialog
      setYamlContent("");
      setFileName("");
      setIsOpen(false);
    } catch (error) {
      console.error("Error uploading YAML:", error);
    } finally {
      setIsSubmitting(false);
    }
  };

  return (
    <Dialog open={isOpen} onOpenChange={setIsOpen}>
      <DialogTrigger asChild>
        <Button variant="outline">
          <Upload className="w-4 h-4 mr-2" />
          Upload YAML
        </Button>
      </DialogTrigger>
      <DialogContent className="min-h-[30vh] min-w-[50vw]">
        <DialogHeader>
          <DialogTitle>Upload and Edit YAML</DialogTitle>
        </DialogHeader>
        <div className="grid gap-4">
          <div className="grid w-full items-center gap-1.5">
            <Input
              type="file"
              accept=".yaml,.yml"
              onChange={handleFileChange}
              className="cursor-pointer file:cursor-pointer file:border-0"
            />
          </div>
          <>
            <YamlEditor
              value={yamlContent}
              onChange={e => {
                setError(null);
                setYamlContent(e);
              }}
            />
            {error && <p className="text-sm text-destructive">{error}</p>}
            <div className="flex justify-end gap-2">
              <Button
                variant="outline"
                onClick={() => {
                  setError(null);
                  setIsOpen(false);
                  setYamlContent("");
                }}
              >
                Cancel
              </Button>
              <Button onClick={handleSubmit} disabled={isSubmitting}>
                {isSubmitting ? "Uploading..." : "Upload"}
              </Button>
            </div>
          </>
        </div>
      </DialogContent>
    </Dialog>
  );
}
