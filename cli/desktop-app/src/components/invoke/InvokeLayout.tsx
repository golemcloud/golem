import { Button } from "@/components/ui/button";
import { ComponentExportFunction, Export, Typ } from "@/types/component.ts";
import { cn } from "@/lib/utils";
import { ClipboardCopy, Presentation, TableIcon } from "lucide-react";
import { DynamicForm } from "@/pages/workers/details/dynamic-form.tsx";
import { SectionCard } from "./SectionCard";
import { parseToJsonEditor, parseTypesData, RawTypesInput } from "@/lib/worker";

export interface InvokeParams {
  params: Array<{
    value: unknown;
    typ: Typ;
    name: string;
  }>;
}

interface InvokeLayoutProps {
  // Navigation data
  parsedExports: Export[];
  name: string;
  urlFn: string;
  onNavigateToFunction: (exportName: string, functionName: string) => void;

  // Function details
  functionDetails: ComponentExportFunction | null;

  // Form state
  viewMode: string;
  setViewMode: (mode: string) => void;
  value: string;
  setValue: (value: string) => void;
  resultValue: string;
  setResultValue: (value: string) => void;

  // Actions
  onValueChange: (value: string) => void;
  onInvoke: (args: InvokeParams) => void;
  copyToClipboard: () => void;
}

export function InvokeLayout({
  parsedExports,
  name,
  urlFn,
  onNavigateToFunction,
  functionDetails,
  viewMode,
  setViewMode,
  value,
  setValue,
  resultValue,
  setResultValue,
  onValueChange,
  onInvoke,
  copyToClipboard,
}: InvokeLayoutProps) {
  return (
    <div className="flex">
      <div className="flex-1 flex flex-col bg-background">
        <div className="flex">
          {/* Sidebar with exports */}
          <div className="border-r px-8 py-4 min-w-[300px]">
            <div className="flex flex-col gap-4 overflow-scroll h-[85vh]">
              {parsedExports.map((exportItem, index) => (
                <div key={exportItem.name + index} className="border-b pb-4">
                  <div className="flex items-center justify-between">
                    <span className="text-sm text-neutral-600 font-bold pb-4">
                      {exportItem.name}
                    </span>
                  </div>
                  <ul className="space-y-1">
                    {(exportItem?.functions || []).length > 0 &&
                      (exportItem.functions || []).map(
                        (fn: ComponentExportFunction) => (
                          <li key={fn.name}>
                            <Button
                              variant="ghost"
                              onClick={() =>
                                onNavigateToFunction(exportItem.name, fn.name)
                              }
                              className={cn(
                                "w-full flex items-center px-3 py-2 rounded-md text-sm font-medium justify-start",
                                urlFn === fn.name
                                  ? "bg-gray-300 dark:bg-neutral-800 text-gray-900 dark:text-gray-100"
                                  : "hover:bg-gray-200 dark:hover:bg-neutral-700 text-gray-700 dark:text-gray-300",
                              )}
                            >
                              <span>{fn.name}</span>
                            </Button>
                          </li>
                        ),
                      )}
                  </ul>
                </div>
              ))}
            </div>
          </div>

          {/* Main content */}
          <div className="flex-1">
            <header className="w-full border-b py-4 px-6">
              <h3>
                {name} - {urlFn}
              </h3>
            </header>

            <div className="p-10 space-y-6 mx-auto overflow-auto h-[80vh]">
              <main className="flex-1 space-y-6">
                {/* View mode buttons */}
                <header className="flex gap-4 items-center mb-4">
                  <div className="flex-1 flex items-center gap-2">
                    <Button
                      variant="outline"
                      onClick={() => {
                        setResultValue("");
                        setViewMode("form");
                      }}
                      className={`text-primary hover:bg-primary/10 hover:text-primary ${
                        viewMode === "form"
                          ? "bg-primary/20 hover:text-primary "
                          : ""
                      }`}
                    >
                      <ClipboardCopy className="h-4 w-4 mr-1" />
                      Form Layout
                    </Button>
                    <Button
                      variant="outline"
                      onClick={() => {
                        setResultValue("");
                        setViewMode("preview");
                      }}
                      className={`text-primary hover:bg-primary/10 hover:text-primary ${
                        viewMode === "preview"
                          ? "bg-primary/20 hover:text-primary "
                          : ""
                      }`}
                    >
                      <Presentation className="h-4 w-4 mr-1" />
                      Json Layout
                    </Button>
                  </div>
                  <div className="flex gap-2">
                    <Button
                      variant="outline"
                      onClick={() => setViewMode("types")}
                      className={`text-primary hover:bg-primary/10 hover:text-primary ${
                        viewMode === "types"
                          ? "bg-primary/20 hover:text-primary "
                          : ""
                      }`}
                    >
                      <TableIcon className="h-4 w-4 mr-1" />
                      Types
                    </Button>
                  </div>
                </header>

                {/* Content based on view mode */}
                {viewMode === "form" && functionDetails && (
                  <DynamicForm
                    functionDetails={functionDetails}
                    onInvoke={data => onInvoke(data as InvokeParams)}
                    resetResult={() => setResultValue("")}
                    exportName={name}
                  />
                )}

                {viewMode === "preview" && functionDetails && (
                  <SectionCard
                    title="Preview"
                    description="Preview the current function invocation arguments"
                    value={
                      value ||
                      JSON.stringify(
                        parseToJsonEditor(functionDetails),
                        null,
                        2,
                      )
                    }
                    onValueChange={onValueChange}
                    copyToClipboard={copyToClipboard}
                    functionDetails={functionDetails}
                    exportName={name}
                    functionName={urlFn}
                    onReset={() => {
                      if (functionDetails) {
                        const initialJson = parseToJsonEditor(functionDetails);
                        setValue(JSON.stringify(initialJson, null, 2));
                      }
                    }}
                    onInvoke={onInvoke}
                  />
                )}

                {viewMode === "types" && functionDetails && (
                  <SectionCard
                    title="Types"
                    description="Types of the function arguments"
                    value={JSON.stringify(
                      parseTypesData(functionDetails as RawTypesInput),
                      null,
                      2,
                    )}
                    functionDetails={functionDetails}
                    exportName={name}
                    functionName={urlFn}
                    copyToClipboard={() => {
                      navigator.clipboard.writeText(
                        JSON.stringify(
                          parseTypesData(functionDetails as RawTypesInput),
                          null,
                          2,
                        ),
                      );
                    }}
                    readOnly={true}
                  />
                )}

                {resultValue && functionDetails && (
                  <SectionCard
                    title="Result"
                    description="View the result of your latest invocation"
                    value={resultValue}
                    readOnly={true}
                    functionDetails={functionDetails}
                    exportName={name}
                    functionName={urlFn}
                  />
                )}
              </main>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
