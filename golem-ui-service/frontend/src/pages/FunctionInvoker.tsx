import {
  AlertCircle,
  ArrowLeft,
  Code2,
  Loader2,
  Play,
  SquareFunction,
  Terminal,
} from "lucide-react";
import { Component, TypeDefinition } from "../types/api";
import { Link, useLocation, useNavigate, useParams } from "react-router-dom";
import { useEffect, useState } from "react";

import RecursiveParameterInput from "../components/shared/RecursiveParameterInput";
import { apiClient } from "../lib/api-client";
import { getComponentVersion } from "../api/components";
import toast from "react-hot-toast";
import { useMutation } from "@tanstack/react-query";
import { useWorker } from "../api/workers";

const FunctionInvoker = () => {
  const { componentId, workerName, componentVersion } = useParams();
  const location = useLocation();
  const queryParams = new URLSearchParams(location.search);
  const functionName = queryParams.get("functionName");
  const exportName = functionName?.split(".")[0];
  const navigate = useNavigate();
  const [component, setComponent] = useState<Component | null>(null);

  const {
    data: worker,
    isLoading: isLoadingWorker,
    error: workerError,
  } = useWorker(componentId!, workerName!);
  const version = worker?.componentVersion || componentVersion;

  useEffect(() => {
    if (componentId && version) {
      getComponentVersion(componentId, version).then(
        (component) => setComponent(component)
      );
    } else if (worker) {
      getComponentVersion(componentId!, worker.componentVersion).then(
        (component) => setComponent(component),
      );
    }
  }, [componentId, version, worker]);

  useEffect(() => {
    if (functionName) {
      document.title = workerName
        ? `Invoke ${functionName} on ${workerName} - Golem UI`
        : `Invoke ${functionName} - Golem UI`;
    }
  }, [functionName, workerName]);

  const [parameters, setParameters] = useState<Record<string, string>>({});
  const [result, setResult] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [isInvoking, setIsInvoking] = useState(false);


  const invokeMutation = useMutation({
    mutationFn: async (params: unknown) => {
      // Choose endpoint based on whether we have a worker
      const endpoint = workerName
        ? `/v1/components/${componentId}/workers/${workerName}/invoke-and-await?function=${functionName}`
        : `/v1/components/${componentId}/invoke?function=${functionName}`;

      const { data } = await apiClient.post(endpoint, params);
      return data;
    },
    onSuccess: (data: string) => {
      setResult(data);
      toast.success("Function invoked successfully");
      setIsInvoking(false);
      setError(null);
    },
    onError: (error: Error) => {
      setError(error.message);
      setIsInvoking(false);
      toast.error(`Failed to invoke function: ${error}`);
    },
  });

  if (isLoadingWorker && workerName) {
    return (
      <div className="flex items-center justify-center h-64">
        <div className="flex items-center gap-2 text-muted-foreground">
          <Loader2 className="animate-spin" size={20} />
          <span>Loading function details...</span>
        </div>
      </div>
    );
  }

  if (workerError && workerName) {
    return (
      <div className="text-center py-12">
        <AlertCircle className="mx-auto h-12 w-12 text-destructive mb-4" />
        <h3 className="text-lg font-semibold mb-2">Worker Not Found</h3>
        <p className="text-muted-foreground">Failed to load worker details.</p>
        <button
          onClick={() => navigate(-1)}
          className="mt-6 text-primary hover:text-primary-accent flex items-center gap-2 mx-auto"
        >
          <ArrowLeft size={16} />
          Go Back
        </button>
      </div>
    );
  }

  const exportDef = component?.metadata?.exports.find(
    (e) => e.name === exportName,
  );
  const functionDef = exportDef?.functions.find(
    (f) =>
      f.name === functionName?.split(".")[1].replace("{", "").replace("}", ""),
  );

  if (!functionDef) {
    return (
      <div className="text-center py-12">
        <AlertCircle className="mx-auto h-12 w-12 text-red-400 mb-4" />
        <h3 className="text-lg font-semibold mb-2">Function Not Found</h3>
        <p className="text-muted-foreground">
          The specified function could not be found.
        </p>
        <button
          onClick={() => navigate(-1)}
          className="mt-6 text-blue-400 hover:text-blue-300 flex items-center gap-2 mx-auto"
        >
          <ArrowLeft size={16} />
          Go Back
        </button>
      </div>
    );
  }

  const handleParameterChange = (path: string, value: unknown) => {
    const updateNestedValue = (
      obj: Record<string, unknown>,
      pathArray: string[],
      value: unknown,
    ): unknown => {
      const [current, ...rest] = pathArray;
      if (rest.length === 0) {
        return { ...obj, [current]: value };
      }
      return {
        ...obj,
        // @ts-expect-error - TS doesn't know that obj[current] is an object
        [current]: updateNestedValue(obj[current] || {}, rest, value),
      };
    };
    // @ts-expect-error - TS doesn't know that parameters is a Record<string, string>
    setParameters((prev) => updateNestedValue(prev, path.split("."), value));
  };

  const transformValue = (value: unknown, typeDef: TypeDefinition): unknown => {
    if (!value) return null;

    switch (typeDef.type) {
      case "List":
        if (!Array.isArray(value)) return [];
        return value.map(item => transformValue(item, typeDef.inner!));

      case "Variant":
        const variantValue = value as { type: string; value: unknown };
        if (!variantValue?.type) return null;

        const selectedCase = typeDef.cases?.find(c => c.name === variantValue.type);
        if (!selectedCase) return null;

        return {
          [variantValue.type]: transformValue(variantValue.value, selectedCase.typ),
        };

      case "Record":
        const recordValue: Record<string, unknown> = {};
        typeDef.fields?.forEach(field => {
          recordValue[field.name] = transformValue(
            (value as Record<string, unknown>)?.[field.name],
            field.typ
          );
        });
        return recordValue;

      case "Option":
        return value === null ? null : transformValue(value, typeDef.inner!);

      default:
        return /f32|f64|u8|u16|u32|u64|i8|i16|i32|i64/i.test(typeDef.type) ? parseFloat(value as string) : value;
    }
  };

  const formatParamsForAPI = () => {
    return {
      params: functionDef.parameters.map((param) => ({
        typ: param.typ,
        value: transformValue(parameters[param.name], param.typ)
      })),
    };
  };

  const handleInvoke = async () => {
    setIsInvoking(true);
    setError(null);
    setResult(null);
    invokeMutation.mutate(formatParamsForAPI());
  };

  return (
    <div className="max-w-4xl mx-auto space-y-6">
      {/* Header */}
      <div className="bg-card/80 rounded-lg p-6">
        <div className="flex items-center gap-4">
          <Link
            to={workerName
              ? `/components/${componentId}/workers/${workerName}`
              : `/components/${componentId}/${version}`
            }
            className="p-2 text-muted-foreground hover:text-foreground rounded-lg hover:bg-card/60"
          >
            <ArrowLeft size={20} />
          </Link>
          <div>
            <h1 className="text-2xl font-bold flex items-center gap-2">
              <SquareFunction className="text-primary" size={24} />
              {functionName}
            </h1>
            <div className="text-muted-foreground mt-1 flex items-center gap-2">
              <span>Export: {exportName}</span>
              {workerName && (
                <>
                  <span>•</span>
                  <span>Worker: {workerName}</span>
                </>
              )}
            </div>
          </div>
        </div>
      </div>

      {/* Parameters */}
      <div className="bg-card/80 rounded-lg p-6">
        <h2 className="text-lg font-semibold flex items-center gap-2 mb-4">
          <Terminal className="text-muted-foreground" size={20} />
          Parameters
        </h2>

        <div className="space-y-4">
          {functionDef.parameters.map((param) => (
            <RecursiveParameterInput
              key={param.name}
              name={param.name}
              typeDef={param.typ}
              value={parameters[param.name]}
              onChange={handleParameterChange}
            />
          ))}

          {functionDef.parameters.length === 0 && (
            <div className="text-center py-4 text-muted-foreground">
              This function takes no parameters
            </div>
          )}
        </div>

        <div className="mt-6 flex justify-end">
          <button
            onClick={handleInvoke}
            disabled={isInvoking || !!error}
            className="flex items-center gap-2 px-4 py-2 bg-blue-500 text-white rounded-lg hover:bg-blue-600 
                     transition-colors disabled:opacity-50"
          >
            {isInvoking ? (
              <>
                <Loader2 size={16} className="animate-spin" />
                Invoking...
              </>
            ) : (
              <>
                <Play size={16} />
                Invoke Function
              </>
            )}
          </button>
        </div>
      </div>

      {/* Results */}
      {(result || error) && (
        <div
          className={`bg-card/80 border rounded-lg p-6 ${error ? "border-destructive/20" : "border-border/10"
            }`}
        >
          <h2 className="text-lg font-semibold flex items-center gap-2 mb-4">
            <Code2
              className={error ? "text-destructive" : "text-primary"}
              size={20}
            />
            {error ? "Error" : "Result"}
          </h2>

          <pre className="bg-card/60 rounded-lg p-4 font-mono text-sm overflow-auto">
            {error || JSON.stringify(result, null, 2)}
          </pre>
        </div>
      )}
    </div>
  );
};

export default FunctionInvoker;
