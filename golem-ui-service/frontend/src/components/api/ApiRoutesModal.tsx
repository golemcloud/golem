import {
  AlertCircle,
  ChevronDown,
  Globe,
  Loader2,
  Route as RouteC,
  Webhook,
  X,
} from "lucide-react";
import { TOOLTIP_CONTENT, Tooltip } from "../shared/Tooltip";
import { useEffect, useState } from "react";

import { Component } from "../../types/api";
import KeyValueInput from "./KeyValueInput";
import RibEditorPanel from "../shared/RibEditorPanel";
import { Route } from "../../pages/ApiDefinitionDetail";
import toast from "react-hot-toast";
import { useComponents } from "../../api/components";
import { useWorkers } from "../../api/workers";

const HTTP_METHODS = [
  { value: "Get", color: "text-green-500 bg-green-500/10" },
  { value: "Post", color: "text-blue-500 bg-primary/10" },
  { value: "Put", color: "text-yellow-500 bg-yellow-500/10" },
  { value: "Delete", color: "text-red-500 bg-red-500/10" },
  { value: "Patch", color: "text-purple-500 bg-purple-500/10" },
  { value: "Head", color: "text-gray-500 bg-gray-500/10" },
  { value: "Options", color: "text-gray-500 bg-gray-500/10" },
];

const BINDING_TYPES = [
  { value: "default", label: "Default" },
  { value: "file-server", label: "File Server" },
  { value: "cors-preflight", label: "CORS Preflight" },
];

// Helper functions moved outside component
const getPathParams = (path: string) => {
  const params: Record<string, { name: string; type: string }> = {};
  const matches = path.match(/{([^}]+)}/g);
  if (matches) {
    matches.forEach((match) => {
      const param = match.replace(/[{}]/g, "");
      params[param] = {
        name: param,
        type: "string",
        documentation: `Path parameter: ${param}`,
      };
    });
  }
  return params;
};

const getContextVariables = (
  pathParams: Record<string, { name: string; type: string }>,
  suggestions: Array<string> = [],
) => [
    {
      name: "request",
      type: "Record",
      documentation: "The incoming HTTP request object.",
      fields: {
        path: {
          name: "path",
          type: "Record",
          documentation: "URL path parameters",
          fields: pathParams,
        },
        body: {
          name: "body",
          type: "any",
          documentation: "The request body content",
        },
        headers: {
          name: "headers",
          type: "Record",
          documentation: "HTTP request headers",
        },
      },
    },
    ...suggestions.map((s) => ({
      name: s,
      type: "string",
      documentation: "Suggestion",
    })),
  ];

interface RouteModalProps {
  isOpen: boolean;
  onClose: () => void;
  onSave: (route: Route) => void;
  existingRoute?: Route | null;
  isLoading: boolean;
}

const Dropdown = ({
  value,
  options,
  onChange,
  className = "",
  placeholder = "Select...",
  error = false,
}: {
  value: string;
  options: { value: string; label: string; disabled?: boolean }[];
  onChange: (value: string) => void;
  className?: string;
  placeholder?: string;
  error?: boolean;
}) => (
  <div className="relative inline-block w-full">
    <select
      value={value}
      onChange={(e) => onChange(e.target.value)}
      className={`w-full pl-3 pr-8 py-2 bg-card/80 rounded-md appearance-none transition-colors
        ${error ? "border-red-500 border-2" : "border border-gray-600"} 
        ${className}`}
    >
      <option value="" disabled>
        {placeholder}
      </option>
      {options.map((opt) => (
        <option key={opt.value} value={opt.value} disabled={opt.disabled}>
          {opt.label}
        </option>
      ))}
    </select>
    <ChevronDown
      className={`absolute right-2 top-1/2 transform -translate-y-1/2 w-4 h-4 
        ${error ? "text-red-500" : "text-muted-foreground"} 
        pointer-events-none`}
    />
    {error && (
      <div className="absolute right-8 top-1/2 transform -translate-y-1/2">
        <AlertCircle className="w-4 h-4 text-red-500" />
      </div>
    )}
  </div>
);

export const RouteModal = ({
  isOpen,
  onClose,
  onSave,
  existingRoute,
  isLoading,
}: RouteModalProps) => {
  const [method, setMethod] = useState("Get");
  const [path, setPath] = useState("");
  const [selectedComponent, setSelectedComponent] = useState<Component>();
  const [selectedVersion, setSelectedVersion] = useState<number>(0);
  const [workerNameScript, setWorkerNameScript] = useState(null);
  const [responseScript, setResponseScript] = useState("");
  const [errors, setErrors] = useState<Record<string, boolean>>({});
  const [bindingType, setBindingType] = useState("default");
  const [corsHeaders, setCorsHeaders] = useState<Record<string, string>>({
    "Access-Control-Allow-Origin": "*",
    "Access-Control-Allow-Methods": "GET, POST, PUT, DELETE, OPTIONS",
    "Access-Control-Allow-Headers": "*",
    "Access-Control-Max-Age": "86400",
    "max-age": "86400",
  });
  const [contextVariables, setContextVariables] = useState(
    getContextVariables(getPathParams("")),
  );
  const [useWorkerName, setUseWorkerName] = useState(true);
  const { data: components } = useComponents();
  const [workerSuggestions, setWorkerSuggestions] = useState<string[]>([]);
  const [exportSuggestions, setExportSuggestions] = useState<
    {
      name: string;
      parameters: { name: string; type: string }[];
    }[]
  >([]);

  const { data: workersData } = useWorkers(
    selectedComponent?.versionedComponentId.componentId || "",
  );

  useEffect(() => {
    const pathParams = getPathParams(path);
    setContextVariables(
      getContextVariables(pathParams, [
        ...workerSuggestions,
        ...exportSuggestions.map(
          (exp) =>
            `${exp.name}(${exp.parameters.map((p) => `${p.name}: ${p.type}`).join(", ")})`,
        ),
      ]),
    );
  }, [path, workerSuggestions, exportSuggestions]);

  useEffect(() => {
    if (existingRoute) {
      setMethod(existingRoute.method);
      setPath(existingRoute.path);
      setWorkerNameScript(existingRoute.binding.workerName);
      setResponseScript(existingRoute.binding.response || "");
      setBindingType(existingRoute.binding.bindingType || "default");
      setSelectedComponent(
        components?.find(
          (c) =>
            c.versionedComponentId.componentId ===
            existingRoute.binding.componentId.componentId &&
            c.versionedComponentId.version ===
            existingRoute.binding.componentId.version,
        ),
      );
      setSelectedVersion(existingRoute.binding.componentId.version);

      if (existingRoute.binding.bindingType === "cors-preflight") {
        const corsResponse = existingRoute.binding.response || "";
        const corsPairs = corsResponse
          .replace(/[{}\s]/g, "")
          .split(",")
          .map((pair) => pair.split(":").map((s) => s.replace(/['"]/g, "")));
        const headers = Object.fromEntries(corsPairs);
        setCorsHeaders(headers);
      } else {
        const corsResponse = existingRoute.binding.corsPreflight || {
          "allowCredentials": true,
          "allowHeaders": "*",
          "allowMethods": "GET, POST, PUT, DELETE, OPTIONS",
          "allowOrigin": "*",
          "maxAge": 0,
        };
        setCorsHeaders(corsResponse);
      }
    }
  }, [existingRoute, components]);

  useEffect(() => {
    if (selectedComponent && workersData) {
      // Extract worker names from active workers
      const workerNames =
        workersData.workers?.map((w) => `"${w.workerId.workerName}"`) || [];
      setWorkerSuggestions(workerNames);

      // Process exports into suggestions with parameters
      const exports = selectedComponent.metadata.exports.flatMap((exp) =>
        exp.functions.map((func) => ({
          name: `${exp.name}.{${func.name}}`,
          parameters: func.parameters.map((x) => ({
            name: x.name,
            type: x.typ.type,
          })),
        })),
      );
      setExportSuggestions(exports);
    }
  }, [selectedComponent, workersData]);

  const validateForm = (): boolean => {
    const newErrors: Record<string, boolean> = {};

    if (!path) newErrors.path = true;
    if (!selectedComponent && bindingType !== "cors-preflight") { newErrors.component = true; }
    // if (!workerNameScript) newErrors.worker = true;

    setErrors(newErrors);
    return Object.keys(newErrors).length === 0;
  };

  const handleSave = () => {
    if (!validateForm()) {
      toast.error("Please fill in all required fields");
      return;
    }

    let finalResponse = responseScript;
    if (bindingType === "cors-preflight") {
      finalResponse = `{
  ${Object.entries(corsHeaders)
          .map(([key, value]) => `${key}: ${JSON.stringify(value)}`)
          .join(",\n  ")}
}`;
    } else {

    }

    const route = {
      method,
      path,
      binding: {
        componentId: bindingType == "cors-preflight" ? null : {
          componentId: selectedComponent!.versionedComponentId.componentId,
          version: selectedVersion,
        },
        workerName: workerNameScript,
        response: finalResponse,
        bindingType,
        corsPreflight: bindingType !== "cors-preflight" ? corsHeaders : undefined
      },
    };


    onSave(route as Route);
  };

  if (!isOpen) return null;

  return (
    <div className="-top-8 fixed overflow-y-scroll inset-0 bg-card bg-opacity-50 flex items-center justify-center p-4 z-50 backdrop-blur-sm">
      <div className="bg-card rounded-lg p-6 max-w-4xl w-full shadow-xl border border-card/85">
        <div className="flex justify-between items-start mb-6">
          <h2 className="text-xl font-semibold flex items-center gap-2">
            <RouteC className="h-5 w-5 text-primary" />
            {existingRoute ? "Edit Route" : "Add New Route"}
          </h2>
          <button
            onClick={onClose}
            className="text-muted-foreground hover:text-gray-300 p-1 rounded-md
              hover:bg-gray-700/50 transition-colors"
          >
            <X size={20} />
          </button>
        </div>

        <div className="space-y-6">
          <div className="grid grid-cols-12 gap-4">
            <div className="col-span-3">
              <label className="block text-sm font-medium mb-1">Method</label>
              <Dropdown
                value={method}
                options={HTTP_METHODS.map((m) => ({
                  value: m.value,
                  label: m.value,
                }))}
                onChange={setMethod}
                placeholder="Select method"
              />
            </div>

            <div className="col-span-6">
              <label className="text-sm font-medium mb-1 flex items-center gap-2">
                Path <span className="text-red-500">*</span>
              </label>
              <div className="relative">
                <Globe
                  className={`absolute left-3 top-2.5 h-4 w-4 
                  ${errors.path ? "text-red-500" : "text-muted-foreground"}`}
                />
                <input
                  type="text"
                  value={path}
                  onChange={(e) => setPath(e.target.value)}
                  className={`bg-card/80 w-full pl-10 pr-3 py-2 rounded-md transition-colors
                    ${errors.path ? "border-2 border-red-500" : "border border-gray-600"}`}
                  placeholder="/api/v1/resource/{id}"
                />
              </div>
            </div>

            <div className="col-span-3">
              <label className="block text-sm font-medium mb-1">
                Binding Type
              </label>
              <Dropdown
                value={bindingType}
                options={BINDING_TYPES}
                onChange={setBindingType}
                placeholder="Select binding type"
              />
            </div>
          </div>

          {bindingType !== "cors-preflight" && <div>
            <label className="block text-sm font-medium mb-1">
              Component <span className="text-red-500">*</span>
            </label>
            <Dropdown
              value={
                selectedComponent
                  ? `${selectedComponent.versionedComponentId.componentId}:${selectedComponent.versionedComponentId.version}`
                  : ""
              }
              options={
                components?.map((c) => ({
                  value: `${c.versionedComponentId.componentId}:${c.versionedComponentId.version}`,
                  label: `${c.componentName} (v${c.versionedComponentId.version})`,
                })) || []
              }
              onChange={(val) => {
                const [cId, version] = val.split(":");
                const component = components?.find(
                  (c) =>
                    c.versionedComponentId.componentId === cId &&
                    c.versionedComponentId.version.toString() === version,
                );
                setSelectedComponent(component);
                setSelectedVersion(Number(version));
              }}
              placeholder="Select component"
              error={errors.component}
            />
          </div>}

          <div className="space-y-4">
            <div className="flex items-center justify-between">
              <label className=" text-sm font-medium mb-1 flex items-center gap-2">
                Worker Name
                <Tooltip
                  content={TOOLTIP_CONTENT.worker.content}
                  title={TOOLTIP_CONTENT.worker.title}
                />
              </label>
              <div className="flex items-center gap-2">
                <span className="text-sm text-muted-foreground">
                  {useWorkerName ? "Use Expression" : "New Ephemeral Worker"}
                </span>
                <button
                  type="button"
                  role="switch"
                  aria-checked={useWorkerName}
                  onClick={() => setUseWorkerName(!useWorkerName)}
                  className={`relative inline-flex h-6 w-11 items-center rounded-full transition-colors 
                   focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary 
                   focus-visible:ring-offset-2 ${useWorkerName ? "bg-primary" : "bg-muted"
                    }`}
                >
                  <span
                    className={`inline-block h-4 w-4 rounded-full bg-white transition-transform 
                     ${useWorkerName ? "translate-x-6" : "translate-x-1"}`}
                  />
                </button>
              </div>
            </div>

            {useWorkerName && path && selectedComponent && (
              <RibEditorPanel
                initialValue={workerNameScript}
                onChange={setWorkerNameScript}
                contextVariables={contextVariables}
                title="Worker Name"
                summary="Define the worker name using a Rib script"
              />
            )}

            <label className=" text-sm font-medium mb-1 flex items-center gap-2">
              Response
              <Tooltip
                content={TOOLTIP_CONTENT.response.content}
                title={TOOLTIP_CONTENT.response.title}
              />
            </label>

            {bindingType !== "cors-preflight" && path && selectedComponent && (
              <RibEditorPanel
                initialValue={responseScript}
                onChange={setResponseScript}
                contextVariables={contextVariables}
                title="Response Transform"
                summary="Define the response transformation using a Rib script"
              />
            )}

            <KeyValueInput
              label="CORS Headers"
              value={corsHeaders}
              onChange={setCorsHeaders}
              editableKeys={false}
            />
          </div>

          <div className="flex justify-end space-x-3 mt-6 pt-4 border-t border-gray-700">
            <button
              onClick={onClose}
              className="px-4 py-2 text-sm bg-gray-700 rounded-md hover:bg-gray-600
                transition-colors"
              disabled={isLoading}
            >
              Cancel
            </button>
            <button
              onClick={handleSave}
              disabled={isLoading}
              className="px-4 py-2 text-sm bg-primary rounded-md hover:bg-blue-600 
                disabled:opacity-50 transition-colors flex items-center gap-2"
            >
              {isLoading ? (
                <>
                  <Loader2 size={16} className="animate-spin" />
                  <span>Saving...</span>
                </>
              ) : (
                <>
                  <Webhook size={16} />
                  <span>Save Route</span>
                </>
              )}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
};

export default RouteModal;
