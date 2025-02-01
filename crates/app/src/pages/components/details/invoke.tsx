import { useCallback, useEffect, useState } from "react";
import { useNavigate, useParams, useSearchParams } from "react-router-dom";
import { API } from "@/service";
import {
  Component,
  ComponentExportFunction,
  Export,
} from "@/types/component.ts";
import ErrorBoundary from "@/components/errorBoundary";
import ComponentLeftNav from "./componentsLeftNav";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { ClipboardCopy } from "lucide-react";
import { cn, sanitizeInput } from "@/lib/utils";
import ReactJson from "react-json-view";
import { Textarea } from "@/components/ui/textarea";
import {
  parseToApiPayload,
  parseToJsonEditor,
  safeFormatJSON,
} from "@/lib/worker";

export default function ComponentInvoke() {
  const { componentId = "" } = useParams();
  const [searchParams] = useSearchParams();
  const navigate = useNavigate();

  const name = searchParams.get("name") || "";
  const urlFn = searchParams.get("fn") || "";

  const [functionDetails, setFunctionDetails] =
    useState<ComponentExportFunction | null>(null);
  const [value, setValue] = useState<string>("{}");
  const [resultValue, setResultValue] = useState<string>("");
  const [error, setError] = useState<string | null>(null);
  const [componentList, setComponentList] = useState<{
    [key: string]: Component;
  }>({});

  /** Fetch function details based on URL params. */
  const fetchFunctionDetails = useCallback(async () => {
    try {
      const data = await API.getComponentByIdAsKey();
      setComponentList(data);
      const matchingComponent = data?.[componentId];
      if (!matchingComponent) {
        throw new Error("Component not found.");
      }
      if (name && urlFn) {
        const exportItem = matchingComponent.exports?.find(
          (e: Export) => e.name === name
        );
        if (!exportItem) {
          throw new Error("Export item not found.");
        }

        const fnDetails = exportItem.functions?.find(
          (f: ComponentExportFunction) => f.name === urlFn
        );
        if (!fnDetails) {
          throw new Error("Function details not found.");
        }
        setFunctionDetails(fnDetails);
        const initialJson = parseToJsonEditor(fnDetails);
        // Pre-format the JSON so it looks nice in the textarea
        setValue(JSON.stringify(initialJson, null, 2));
      } else if (
        !name &&
        !urlFn &&
        matchingComponent?.exports?.[0]?.functions?.[0]
      ) {
        navigate(
          `/components/${componentId}/invoke?name=${matchingComponent.exports[0].name}&&fn=${matchingComponent.exports[0].functions[0].name}`
        );
      }
    } catch (error: unknown) {
      if (error instanceof Error) {
        setError(error.message);
      } else {
        setError("Unable to fetch function details.");
      }
    }
  }, [componentId, urlFn, name]);

  useEffect(() => {
    if (componentId) {
      setError(null);
      setResultValue("");
      fetchFunctionDetails();
    }
  }, [componentId, name, urlFn, fetchFunctionDetails]);

  const handleValueChange = (newValue: string) => {
    const formatted = safeFormatJSON(newValue);
    setValue(formatted);
    setResultValue("");
    setError(null);
  };

  const onInvoke = async () => {
    try {
      setError(null);
      const sanitizedValue = sanitizeInput(value);
      const parsedValue = JSON.parse(sanitizedValue);

      if (!functionDetails) {
        throw new Error("No function details loaded.");
      }

      const apiData = parseToApiPayload(parsedValue, functionDetails);

      const functionName = `${encodeURIComponent(name)}.${encodeURIComponent(
        `{${urlFn}}`
      )}`;
      const response = await API.invokeEphemeralAwait(
        componentId,
        functionName,
        apiData
      );

      const newValue = JSON.stringify(response?.result?.value, null, 2);
      setResultValue(newValue);
    } catch (error: unknown) {
      if (error instanceof Error) {
        setError(error.message);
      } else {
        setError("Invalid JSON data. Please correct it before invoking.");
      }
    }
  };

  const copyToClipboard = () => {
    navigator.clipboard.writeText(value);
  };

  return (
    <ErrorBoundary>
      <div className="flex h-screen">
        <ComponentLeftNav componentDetails={componentList[componentId]} />
        <div className="flex-1 flex flex-col bg-background">
          <div className="flex">
            <div className="border-r px-8 py-4 min-w-[300px]">
              <div className="grid grid-cols-1 gap-4 overflow-scroll h-[80vh]">
                {componentList?.[componentId]?.exports?.map((exportItem) => (
                  <div key={exportItem.name}>
                    <div className="flex items-center justify-between">
                      <span className="text-sm text-gray-600 font-bold pb-4">
                        {exportItem.name}
                      </span>
                    </div>
                    <ul className="space-y-1">
                      {exportItem?.functions?.length > 0 &&
                        exportItem.functions.map(
                          (fn: ComponentExportFunction) => (
                            <li key={fn.name}>
                              <Button
                                variant="ghost"
                                onClick={() =>
                                  navigate(
                                    `/components/${componentId}/invoke?name=${exportItem.name}&&fn=${fn.name}`
                                  )
                                }
                                className={cn(
                                  "w-full flex items-center px-3 py-2 rounded-md text-sm font-medium justify-start",
                                  urlFn === fn.name
                                    ? "bg-gray-200 dark:bg-gray-800 text-gray-900 dark:text-gray-100"
                                    : "hover:bg-gray-100 dark:hover:bg-gray-900 text-gray-600 dark:text-gray-400"
                                )}
                              >
                                <span>{fn.name}</span>
                              </Button>
                            </li>
                          )
                        )}
                    </ul>
                  </div>
                ))}
              </div>
            </div>
            <div className="flex-1">
              <header className="w-full border-b py-4 px-6">
                <h3>
                  {name} - {urlFn}
                </h3>
              </header>

              <div className="p-10 space-y-6 mx-auto overflow-auto h-[80vh] w-[60%]">
                <main className="flex-1 p-6 space-y-6">
                  <SectionCard
                    title="Preview"
                    description="Preview the current function invocation arguments"
                    value={value}
                    onValueChange={handleValueChange}
                    copyToClipboard={copyToClipboard}
                    error={error}
                  />

                  <div className="flex justify-end">
                    <Button onClick={onInvoke} className="px-6">
                      Invoke
                    </Button>
                  </div>

                  {resultValue && (
                    <SectionCard
                      title="Result"
                      description="View the result of your latest invocation"
                      value={resultValue}
                      readOnly
                    />
                  )}
                </main>
              </div>
            </div>
          </div>
        </div>
      </div>
    </ErrorBoundary>
  );
}

/* ----------------------------------
 * Reusable SectionCard Component
 * ---------------------------------- */

interface SectionCardProps {
  title: string;
  description: string;
  value: string;
  onValueChange?: (value: string) => void;
  copyToClipboard?: () => void;
  error?: string | null;
  readOnly?: boolean;
}

function SectionCard({
  title,
  description,
  value,
  onValueChange,
  copyToClipboard,
  error,
  readOnly = false,
}: SectionCardProps) {
  return (
    <Card className="w-full bg-background">
      <CardHeader className="flex items-center pb-2 flex-row">
        <div className="flex items-center justify-between w-full">
          <div>
            <CardTitle className="text-xl font-bold">{title}</CardTitle>
            <p className="text-sm text-muted-foreground">{description}</p>
          </div>
          {copyToClipboard && (
            <Button variant="outline" size="sm" onClick={copyToClipboard}>
              <ClipboardCopy className="h-4 w-4 mr-1" />
              Copy
            </Button>
          )}
        </div>
      </CardHeader>
      <CardContent>
        {readOnly ? (
          <ReactJson
            src={JSON.parse(value || "{}")}
            name={null}
            theme="rjv-default"
            collapsed={false}
            enableClipboard={false}
            displayDataTypes={false}
            style={{ fontSize: "14px", lineHeight: "1.6" }}
          />
        ) : (
          <Textarea
            value={value}
            onChange={(e) => onValueChange?.(e.target.value)}
            className={cn(
              "min-h-[200px] font-mono text-sm mt-2",
              error && "border-red-500"
            )}
            placeholder="Enter JSON data..."
          />
        )}
        {error && <p className="text-red-500 text-sm mt-2">{error}</p>}
      </CardContent>
    </Card>
  );
}
