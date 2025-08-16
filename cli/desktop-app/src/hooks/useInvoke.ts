import { useCallback, useEffect, useState } from "react";
import { useNavigate, useParams, useSearchParams } from "react-router-dom";
import { API } from "@/service";
import {
  ComponentExportFunction,
  ComponentList,
  parseExportString,
  Typ,
} from "@/types/component.ts";
import { parseToJsonEditor, safeFormatJSON } from "@/lib/worker";
import { toast } from "@/hooks/use-toast";

interface UseInvokeProps {
  isWorkerInvoke?: boolean;
}

export interface InvokeResponse {
  result_json: Record<string, unknown>;
}

export function useInvoke({ isWorkerInvoke = false }: UseInvokeProps = {}) {
  const { componentId = "", appId, workerName } = useParams();
  const [searchParams] = useSearchParams();
  const navigate = useNavigate();

  const name = searchParams.get("name") || "";
  const urlFn = searchParams.get("fn") || "";

  const [functionDetails, setFunctionDetails] =
    useState<ComponentExportFunction | null>(null);
  const [value, setValue] = useState<string>("{}");
  const [resultValue, setResultValue] = useState<string>("");
  const [componentList, setComponentList] = useState<{
    [key: string]: ComponentList;
  }>({});
  const [viewMode, setViewMode] = useState("form");

  const fetchFunctionDetails = useCallback(async () => {
    try {
      const data = await API.componentService.getComponentByIdAsKey(appId!);
      setComponentList(data);
      const matchingComponent =
        data?.[componentId]?.versions?.[
          data?.[componentId].versions.length - 1
        ];

      if (!matchingComponent) {
        throw new Error("Component not found.");
      }

      // Parse exports using the new parser
      matchingComponent.parsedExports = (matchingComponent?.exports || [])
        .map(parseExportString)
        .filter(x => !!x);

      if (name && urlFn) {
        let exportItem;
        let fnDetails;

        if (isWorkerInvoke && matchingComponent.parsedExports) {
          // Workers version - use parsed exports
          exportItem = matchingComponent.parsedExports.find(
            e => e.name === name && e.functions.some(f => f.name === urlFn),
          );
          if (!exportItem) {
            throw new Error("Export item not found.");
          }

          fnDetails = exportItem.functions?.find(
            (f: ComponentExportFunction) => f.name === urlFn,
          );
        } else {
          // Components version - find export string and parse it
          const exportItemText = matchingComponent?.exports?.find(
            (item: string) => name === item,
          );
          if (!exportItemText) {
            throw new Error("Export item not found.");
          }
          exportItem = parseExportString(exportItemText);
          fnDetails = exportItem?.functions?.find(
            (f: ComponentExportFunction) => f.name === urlFn,
          );
        }

        if (!fnDetails) {
          throw new Error("Function details not found.");
        }

        setFunctionDetails(fnDetails);
        const initialJson = parseToJsonEditor(fnDetails);
        setValue(JSON.stringify(initialJson, null, 2));
      } else if (
        !name &&
        !urlFn &&
        matchingComponent.parsedExports?.[0]?.functions?.[0]
      ) {
        // Navigate to first available function
        const firstExport = matchingComponent.parsedExports[0];
        const path = isWorkerInvoke
          ? `/app/${appId}/components/${componentId}/workers/${workerName}/invoke?name=${firstExport.name}&&fn=${firstExport.functions[0]?.name}`
          : `/app/${appId}/components/${componentId}/invoke?name=${firstExport.name}&&fn=${firstExport.functions[0]?.name}`;
        navigate(path);
      }
    } catch (error: unknown) {
      if (error instanceof Error) {
        toast({
          title: error.message,
          variant: "destructive",
          duration: Number.POSITIVE_INFINITY,
        });
      } else {
        toast({
          title: "Unable to fetch function details.",
          variant: "destructive",
          duration: Number.POSITIVE_INFINITY,
        });
      }
    }
  }, [componentId, urlFn, name, workerName, appId, isWorkerInvoke, navigate]);

  useEffect(() => {
    if (componentId) {
      setResultValue("");
      fetchFunctionDetails();
    }
  }, [componentId, name, urlFn, fetchFunctionDetails]);

  const handleValueChange = (newValue: string) => {
    const formatted = safeFormatJSON(newValue);
    setValue(formatted);
    setResultValue("");
  };

  const onInvoke = async (
    parsedValue:
      | unknown[]
      | { params: Array<{ value: unknown; typ: Typ; name: string }> },
  ) => {
    try {
      if (!functionDetails) {
        throw new Error("No function details loaded.");
      }

      let params: Array<{ value: unknown; typ: Typ; name?: string }>;

      // Handle both old format (array) and new format (object with params)
      if (Array.isArray(parsedValue)) {
        // Old format - convert to new format
        params = parsedValue.map((value, index) => ({
          value,
          typ: functionDetails.parameters[index]?.typ!,
          name: functionDetails.parameters[index]?.name,
        }));
      } else {
        // New format - use directly
        params = parsedValue.params;
      }

      const functionName = `${name}.{${urlFn}}`;
      let response: InvokeResponse;

      if (isWorkerInvoke) {
        response = await API.workerService.invokeWorkerAwait(
          appId!,
          componentId,
          workerName!,
          functionName,
          { params },
        );
      } else {
        response = await API.workerService.invokeEphemeralAwait(
          appId!,
          componentId,
          functionName,
          {
            params,
          },
        );
      }

      const newValue = JSON.stringify(response?.result_json, null, 2);
      setResultValue(newValue);
    } catch (error: unknown) {
      if (
        typeof error === "object" &&
        error !== null &&
        "description" in error
      ) {
        const description = (error as { description?: string }).description;
        toast({
          title: description ?? "An unknown error occurred.",
          variant: "destructive",
        });
      } else if (typeof error === "string" || typeof error === "object") {
        toast({
          title: String(error),
          variant: "destructive",
        });
      }
    }
  };

  const copyToClipboard = () => {
    navigator.clipboard.writeText(value);
  };

  const componentDetails =
    componentList[componentId]?.versions?.[
      componentList[componentId]?.versions.length - 1
    ] || {};
  const parsedExports = (componentDetails?.exports || [])
    ?.map(parseExportString)
    .filter(x => !!x);

  return {
    // State
    functionDetails,
    value,
    setValue,
    resultValue,
    setResultValue,
    componentList,
    viewMode,
    setViewMode,
    componentDetails,
    parsedExports,

    // Computed values
    name,
    urlFn,
    appId,
    componentId,
    workerName,

    // Functions
    handleValueChange,
    onInvoke,
    copyToClipboard,
    navigate,
  };
}
