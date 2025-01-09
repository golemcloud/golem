import {
  AlertCircle,
  Box,
  ChevronDown,
  Code2,
  Globe,
  HelpCircle,
  Loader2,
  Route as RouteC,
  Webhook,
  X,
} from "lucide-react";
import { useEffect, useRef, useState } from "react";

import { Component } from "../../types/api";
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

const RIB_TYPES = [
  {
    category: "Basic Types",
    types: [
      { value: "string", label: "string" },
      { value: "bool", label: "bool" },
      { value: "char", label: "char" },
      { value: "s8", label: "s8 (signed 8-bit)" },
      { value: "u8", label: "u8 (unsigned 8-bit)" },
      { value: "s16", label: "s16 (signed 16-bit)" },
      { value: "u16", label: "u16 (unsigned 16-bit)" },
      { value: "s32", label: "s32 (signed 32-bit)" },
      { value: "u32", label: "u32 (unsigned 32-bit)" },
      { value: "s64", label: "s64 (signed 64-bit)" },
      { value: "u64", label: "u64 (unsigned 64-bit)" },
      { value: "f32", label: "f32 (32-bit float)" },
      { value: "f64", label: "f64 (64-bit float)" },
    ],
  },
  {
    category: "Complex Types",
    types: [
      { value: "list<string>", label: "list<string>" },
      { value: "tuple<string,u32>", label: "tuple<string,u32>" },
      { value: "option<string>", label: "option<string>" },
      { value: "result<string,string>", label: "result<string,string>" },
    ],
  },
];

const NUMERIC_TYPES = [
  "s8",
  "u8",
  "s16",
  "u16",
  "s32",
  "u32",
  "s64",
  "u64",
  "f32",
  "f64",
];

const TOOLTIP_CONTENT = {
  path: {
    title: "Path Parameters",
    content: `<pre class="bg-gray-900 p-2 rounded">{&lt;VARIABLE_NAME&gt;}</pre>`,
  },
  worker: {
    title: "Common Interpolation Expressions",
    content: `
      <div class="space-y-3">
        <div>
          <div class="font-medium mb-1">Path Parameters:</div>
          <pre class="bg-gray-900 p-2 rounded mb-1">\${request.path.&lt;PATH_PARAM_NAME&gt;}</pre>
        </div>
        <div>
          <div class="font-medium mb-1">Query Parameters:</div>
          <pre class="bg-gray-900 p-2 rounded mb-1">\${request.path.&lt;QUERY_PARAM_NAME&gt;}</pre>
        </div>
        <div>
          <div class="font-medium mb-1">Request Body:</div>
          <pre class="bg-gray-900 p-2 rounded mb-1">\${request.body}</pre>
        </div>
        <div>
          <div class="font-medium mb-1">Request Body Field:</div>
          <pre class="bg-gray-900 p-2 rounded mb-1">\${request.body.&lt;FIELD_NAME&gt;}</pre>
        </div>
        <div>
          <div class="font-medium mb-1">Request Headers:</div>
          <pre class="bg-gray-900 p-2 rounded mb-1">\${request.header.&lt;HEADER_NAME&gt;}</pre>
        </div>
      </div>
    `,
  },
};


const Tooltip = ({ content, title }: { content: string, title: string }) => {
  const [isOpen, setIsOpen] = useState(false);
  const tooltipRef = useRef<HTMLDivElement>(null);
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    const handleClickOutside = (event: MouseEvent) => {
      if (tooltipRef.current && !tooltipRef.current.contains(event.target as Node)) {
        setIsOpen(false);
      }
    };

    document.addEventListener('mousedown', handleClickOutside);
    return () => document.removeEventListener('mousedown', handleClickOutside);
  }, []);


  return (
    <div className="relative inline-block">
      <button
        onClick={() => setIsOpen(!isOpen)}
      >
        <HelpCircle
          className={`w-4 h-4 cursor-pointer transition-colors ${isOpen ? 'text-primary' : 'text-muted-foreground hover:text-gray-300'
            }`}

        /></button>
      {isOpen && (
        <div
          ref={tooltipRef}
          className="absolute left-full ml-2 w-96 p-4 bg-gray-800 rounded-lg shadow-xl 
            text-sm z-50 border border-gray-700"
        >
          <div className="flex justify-between items-start mb-3">
            <h3 className="font-medium text-base">{title}</h3>
            <button
              onClick={() => setIsOpen(false)}
              className="text-gray-400 hover:text-gray-300 p-1 rounded-md hover:bg-gray-700"
            >
              <X size={14} />
            </button>
          </div>
          <div
            className="text-gray-300 space-y-2"
            dangerouslySetInnerHTML={{ __html: content }}
          />
        </div>
      )}
    </div>
  );
};

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
  <div className='relative inline-block w-full'>
    <select
      value={value}
      onChange={(e) => onChange(e.target.value)}
      className={`w-full pl-3 pr-8 py-2 bg-card/80 rounded-md appearance-none transition-colors
        ${error ? "border-red-500 border-2" : "border border-gray-600"} 
        ${className}`}>
      <option value='' disabled>
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
      <div className='absolute right-8 top-1/2 transform -translate-y-1/2'>
        <AlertCircle className='w-4 h-4 text-red-500' />
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
  const [selectedWorker, setSelectedWorker] = useState("");
  const [response, setResponse] = useState("");
  const [selectedRibType, setSelectedRibType] = useState("");
  const [errors, setErrors] = useState<Record<string, boolean>>({});
  const [customWorkerExpression, setCustomWorkerExpression] = useState("");
  const [isCustomWorker, setIsCustomWorker] = useState(false);
  const [isCustomResponse, setIsCustomResponse] = useState(false);
  const [selectedFunction, setSelectedFunction] = useState("");

  const [bindingType, setBindingType] = useState("default");
  const { data: components } = useComponents();
  const { data: workersData } = useWorkers(
    selectedComponent?.versionedComponentId.componentId || ""
  );
  const availableFunctions = selectedComponent?.metadata.exports.flatMap(exp =>
    exp.functions.map(fn => ({
      value: `golem:component/api.${fn.name}`,
      label: `${exp.name}.${fn.name}`
    }))
  ) || [];

  const workerOptions =
    workersData?.workers.map((w) => ({
      value: w.workerId.workerName,
      label: w.workerId.workerName,
    })) || [];

  useEffect(() => {
    if (existingRoute) {
      setMethod(existingRoute.method);
      setPath(existingRoute.path);
      setSelectedWorker(existingRoute.binding.workerName);
      setBindingType(existingRoute.binding.bindingType || "default");
      setResponse(existingRoute.binding.response!);
      setSelectedComponent(
        components?.find(
          (c) =>
            c.versionedComponentId.componentId ===
            existingRoute.binding.componentId.componentId &&
            c.versionedComponentId.version ===
            existingRoute.binding.componentId.version
        )
      );
      setSelectedVersion(existingRoute.binding.componentId.version);

      const isExpression = existingRoute.binding.workerName.includes("${");
      setIsCustomWorker(isExpression);
      if (isExpression) {
        setCustomWorkerExpression(existingRoute.binding.workerName);
        setSelectedWorker("");
      } else {
        setSelectedWorker(existingRoute.binding.workerName);
        setCustomWorkerExpression("");
      }

      const isResponseFunction = existingRoute.binding.response?.includes('golem:component');
      setIsCustomResponse(isResponseFunction!);

      if (isResponseFunction) {
        setSelectedFunction(existingRoute.binding.response?.replace(/['"]/g, '') || '');
        setResponse('');
      } else {
        setResponse(existingRoute.binding.response!);
        setSelectedFunction('');
      }

    }
  }, [existingRoute, components]);

  useEffect(() => {
    // Set initial response based on binding type
    if (bindingType === "file-server" && !response) {
      setResponse('let file: string = request.path.file; "/files/${{file}}"');
    } else if (bindingType === "cors-preflight" && !response) {
      setResponse(`
  {
                  Access-Control-Allow-Origin: "{}",
                  Access-Control-Allow-Methods: "{}",
                  Access-Control-Allow-Headers: "{}",
                  Access-Control-Expose-Headers: "{}",
                  Access-Control-Allow-Credentials: {},
                  Access-Control-Max-Age: {}u64
}
  `);
    }
  }, [bindingType]);

  useEffect(() => {
    // Reset errors when fields change
    setErrors({});
  }, [path, selectedComponent, selectedWorker, response]);

  const formatResponse = (value: string, type: string): string => {
    if (!value) return "";
    if (NUMERIC_TYPES.includes(type)) {
      // For numeric types, append the type suffix
      return `${value}${type}`;
    }
    return value;
  };


  const formatSelectedWorker = (value: string): string => {
    if (!value) return "";
    if (value.startsWith('"')) {
      return value;
    }
    return `"${value}"`;
  }

  const stripNumSuffix = (value: string): number => {
    if (!value) return 0;
    let suffix = NUMERIC_TYPES.filter((t) => value.endsWith(t))[0]
    if (suffix) {
      return Number(value.slice(0, -suffix.length));
    }
    return Number(value);
  };

  const validateForm = (): boolean => {
    const newErrors: Record<string, boolean> = {};

    if (!path) newErrors.path = true;
    if (!selectedComponent) newErrors.component = true;
    if (!selectedWorker && !customWorkerExpression) newErrors.worker = true;
    if (selectedRibType && !response && !isCustomResponse) newErrors.response = true;

    setErrors(newErrors);
    return Object.keys(newErrors).length === 0;
  };

  const handleSave = () => {
    if (!validateForm()) {
      toast.error("Please fill in all required fields");
      return;
    }

    const finalWorkerName = isCustomWorker ? customWorkerExpression : formatSelectedWorker(selectedWorker);

    let finalResponse = '';
    if (selectedRibType) {
      if (isCustomResponse) {
        finalResponse = selectedFunction;
      } else {
        finalResponse = formatResponse(response, selectedRibType);
      }
    } else {
      finalResponse = response;
    }

    const route = {
      method,
      path,
      binding: {
        componentId: {
          componentId: selectedComponent!.versionedComponentId.componentId,
          version: selectedVersion,
        },
        workerName: finalWorkerName,
        response: finalResponse,
        bindingType
      },
    };

    onSave(route as Route);
  };

  if (!isOpen) return null;

  return (
    <div className='fixed inset-0 bg-card bg-opacity-50 flex items-center justify-center p-4 z-50 backdrop-blur-sm'>
      <div className='bg-card rounded-lg p-6 max-w-4xl w-full shadow-xl border border-card/85'>
        <div className='flex justify-between items-start mb-6'>
          <h2 className='text-xl font-semibold flex items-center gap-2'>
            <RouteC className='h-5 w-5 text-primary' />
            {existingRoute ? "Edit Route" : "Add New Route"}
          </h2>
          <button
            onClick={onClose}
            className='text-muted-foreground hover:text-gray-300 p-1 rounded-md
              hover:bg-gray-700/50 transition-colors'>
            <X size={20} />
          </button>
        </div>

        <div className='space-y-6'>
          <div className='grid grid-cols-12 gap-4'>
            <div className='col-span-3'>
              <label className='block text-sm font-medium mb-1'>Method</label>
              <Dropdown
                value={method}
                options={HTTP_METHODS.map((m) => ({
                  value: m.value,
                  label: m.value,
                }))}
                onChange={setMethod}
                placeholder='Select method'
              />
            </div>

            <div className="col-span-6">
              <label className="text-sm font-medium mb-1 flex items-center gap-2">
                Path <span className="text-red-500">*</span>
                <Tooltip content={TOOLTIP_CONTENT.path.content} title={TOOLTIP_CONTENT.path.title} />
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

            <div className='col-span-3'>
              <label className='block text-sm font-medium mb-1'>Binding Type</label>
              <Dropdown
                value={bindingType}
                options={BINDING_TYPES}
                onChange={setBindingType}
                placeholder='Select binding type'
              />
            </div>
          </div>

          <div>
            <label className='block text-sm font-medium mb-1'>
              Component <span className='text-red-500'>*</span>
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
                    c.versionedComponentId.version.toString() === version
                );
                setSelectedComponent(component);
                setSelectedVersion(Number(version));
              }}
              placeholder='Select component'
              error={errors.component}
            />
          </div>

          <div>
            <label className=" text-sm font-medium mb-1 flex items-center justify-between">
              <div className="flex items-center gap-2">
                Worker <span className="text-red-500">*</span>
                <Tooltip content={TOOLTIP_CONTENT.worker.content} title={TOOLTIP_CONTENT.worker.title} />
              </div>
              <div className="flex items-center gap-2 text-sm font-normal">
                <label className="flex items-center gap-1.5">
                  <input
                    type="radio"
                    checked={!isCustomWorker}
                    onChange={() => {
                      setIsCustomWorker(false);
                      setCustomWorkerExpression("");
                    }}
                    className="text-primary"
                  />
                  Select Worker
                </label>
                <label className="flex items-center gap-1.5">
                  <input
                    type="radio"
                    checked={isCustomWorker}
                    onChange={() => {
                      setIsCustomWorker(true);
                      setSelectedWorker("");
                    }}
                    className="text-primary"
                  />
                  Expression
                </label>
              </div>
            </label>
            <div className="space-y-2">
              {!isCustomWorker ? (
                <Dropdown
                  value={selectedWorker}
                  options={workerOptions}
                  onChange={setSelectedWorker}
                  placeholder="Select worker"
                  error={errors.worker && !isCustomWorker}
                />
              ) : (
                <div className="relative">
                  <Code2 className={`absolute left-3 top-2.5 h-4 w-4 
                    ${errors.worker ? "text-red-500" : "text-muted-foreground"}`} />
                  <textarea
                    value={customWorkerExpression}
                    onChange={(e) => setCustomWorkerExpression(e.target.value)}
                    className={`bg-card/80 w-full pl-10 pr-3 py-2 rounded-md transition-colors
                      ${errors.worker && isCustomWorker ? "border-2 border-red-500" : "border border-gray-600"}`}
                    placeholder="${request.path.worker_name}"
                  />
                </div>
              )}
            </div>
          </div>

          <div className='space-y-4'>
            <div>
              <label className="text-sm font-medium mb-1 flex items-center justify-between">
                <label className='block text-sm font-medium mb-1'>
                  Response Type
                </label>
                <div className="flex items-center gap-2 text-sm font-normal">
                  <label className="flex items-center gap-1.5">
                    <input
                      type="radio"
                      checked={!isCustomResponse}
                      onChange={() => {
                        setIsCustomResponse(false);
                        setSelectedFunction("");
                      }}
                      className="text-primary"
                    />
                    Value
                  </label>
                  <label className="flex items-center gap-1.5">
                    <input
                      type="radio"
                      checked={isCustomResponse}
                      onChange={() => {
                        setIsCustomResponse(true);
                        setResponse("");
                      }}
                      className="text-primary"
                    />
                    Function
                  </label>
                </div>
              </label>
              {!isCustomResponse && bindingType === 'default' && (
                <Dropdown
                  value={selectedRibType}
                  options={RIB_TYPES.reduce(
                    (acc, category) => [
                      ...acc,
                      {
                        value: category.category,
                        label: category.category,
                        disabled: true,
                      },
                      ...category.types,
                    ],
                    []
                  )}
                  onChange={(type) => {
                    setSelectedRibType(type);
                    if (NUMERIC_TYPES.includes(type) && response) {
                      setResponse("");
                    }
                  }}
                  placeholder='Select Rib type'
                />
              )}
            </div>

            {(selectedRibType || bindingType !== 'default') && (
              <div>
                <label className="text-sm font-medium mb-1 flex items-center justify-between">
                  <div className="flex items-center gap-2">
                    Response {selectedRibType && <span className="text-red-500">*</span>}
                  </div>
                </label>

                {isCustomResponse ? (
                  <Dropdown
                    value={selectedFunction}
                    options={availableFunctions}
                    onChange={setSelectedFunction}
                    placeholder="Select function"
                    error={errors.response && isCustomResponse}
                  />
                ) : bindingType !== 'default' || typeof response === 'string' ? (
                  <textarea
                    value={response}
                    onChange={(e) => setResponse(e.target.value)}
                    className={`bg-card/80 w-full p-3 rounded-md transition-colors font-mono text-sm h-40
                      ${errors.response ? "border-2 border-red-500" : "border border-gray-600"}`}
                    placeholder={`Enter response ${bindingType !== 'default' ? 'configuration' : 'value'}`}
                  />
                ) : (
                  <div className='relative'>
                    <Code2
                      className={`absolute left-3 top-2.5 h-4 w-4 
                      ${errors.response ? "text-red-500" : "text-muted-foreground"}`}
                    />
                    <input
                      type={NUMERIC_TYPES.includes(selectedRibType) ? "number" : "text"}
                      value={stripNumSuffix(response)}
                      onChange={(e) => setResponse(e.target.value)}
                      className={`bg-card/80 w-full pl-10 pr-3 py-2 rounded-md transition-colors
                      ${errors.response ? "border-2 border-red-500" : "border border-gray-600"}`}
                      placeholder={`Enter ${selectedRibType} value`}
                    />
                  </div>
                )}
              </div>
            )}
          </div>


          <div className='flex justify-end space-x-3 mt-6 pt-4 border-t border-gray-700'>
            <button
              onClick={onClose}
              className='px-4 py-2 text-sm bg-gray-700 rounded-md hover:bg-gray-600
                transition-colors'
              disabled={isLoading}>
              Cancel
            </button>
            <button
              onClick={handleSave}
              disabled={isLoading}
              className='px-4 py-2 text-sm bg-primary rounded-md hover:bg-blue-600 
                disabled:opacity-50 transition-colors flex items-center gap-2'>
              {isLoading ? (
                <>
                  <Loader2 size={16} className='animate-spin' />
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
