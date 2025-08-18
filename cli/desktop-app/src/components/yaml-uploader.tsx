// @ts-nocheck
import * as yaml from "js-yaml";

import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";
import { useEffect, useState } from "react";
import { useNavigate, useParams, useSearchParams } from "react-router-dom";

import { API } from "@/service";
import { Api } from "@/types/api.ts";
import { Button } from "@/components/ui/button";
import { ENDPOINT } from "@/service/endpoints.ts";
import { Input } from "@/components/ui/input.tsx";
import { Upload } from "lucide-react";
import { YamlEditor } from "./yaml-editor";

// import { parse } from "path";

type ValidationError = string;

export default function YamlUploader() {
  const { apiName, version, appId } = useParams();
  const navigate = useNavigate();
  const [queryParams] = useSearchParams();
  const path = queryParams.get("path");
  const method = queryParams.get("method");
  const reload = queryParams.get("reload");
  const [_isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [yamlContent, setYamlContent] = useState<string>("");
  const [_fileName, setFileName] = useState<string>("");
  const [isOpen, setIsOpen] = useState(false);
  const [isSubmitting, setIsSubmitting] = useState(false);
  const [activeApiDetails, setActiveApiDetails] = useState<Api | null>(null);

  useEffect(() => {
    const fetchData = async () => {
      if (!apiName) return;
      try {
        setIsLoading(true);
        const [apiResponse, _componentResponse] = await Promise.all([
          API.apiService.getApi(appId, apiName),
          API.componentService.getComponentByIdAsKey(appId!),
        ]);
        setActiveApiDetails(apiResponse!);
      } catch (_error) {
        console.error("Failed to fetch data:", _error);
        setError("Failed to load required data. Please try again.");
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
    } catch {
      setError("Invalid YAML file.");
      // You might want to show an error toast or message here
    }
  };

  const onSubmit = async (payload: unknown) => {
    try {
      setIsSubmitting(true);
      await API.apiService
        .callApi(
          ENDPOINT.putApi(activeApiDetails?.id!, version!),
          "PUT",
          payload,
          { "Content-Type": "application/yaml" },
        )
        .then(() => {
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
    let parsedData: unknown;
    const errors: ValidationError[] = [];

    // Step 1: Parse YAML to JSON
    try {
      parsedData = yaml.load(yamlString);
    } catch {
      return {
        isValid: false,
        errors: ["Invalid YAML format."],
      };
    }

    // Step 2: Validate main API structure
    if (!parsedData.id || typeof parsedData.id !== "string") {
      errors.push("Invalid or missing 'id' field.");
    }

    // id must match the apiName
    if (parsedData.id !== activeApiDetails?.id) {
      errors.push(`'id' field must match the API name: ${apiName}.`);
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
      parsedData.routes.forEach((route: Route) => {
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
          const { type: bindingType, component, response } = route.binding;

          if (
            !["default", "file-server", "cors-preflight"].includes(bindingType)
          ) {
            errors.push("Invalid 'bindingType'.");
          }

          if (bindingType === "cors-preflight") {
            if (!component || typeof component !== "object") {
              errors.push("Missing 'component' for 'cors-preflight' binding.");
            } else {
              if (!component.name || typeof component.name !== "string") {
                errors.push("Invalid 'component.name'.");
              }

              if (typeof component.version !== "number") {
                errors.push("Invalid 'component.version'.");
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
