import { useEffect, useState } from "react";
import { ComponentExportFunction, Typ, Field } from "@/types/component";
import { Card, CardContent } from "@/components/ui/card";
import { CircleSlash2, Info, Play, TimerReset } from "lucide-react";
import { Button } from "@/components/ui/button";
import { canInvokeHttpHandler } from "@/lib/http-handler";
import { RecursiveParameterInput } from "@/components/invoke/RecursiveParameterInput";

export const nonStringPrimitives = [
  "S64",
  "S32",
  "S16",
  "S8",
  "U64",
  "U32",
  "U16",
  "U8",
  "Bool",
  "Enum",
];

export const DynamicForm = ({
  functionDetails,
  onInvoke,
  resetResult,
  exportName = "",
}: {
  functionDetails: ComponentExportFunction;
  onInvoke: (
    args:
      | unknown[]
      | { params: Array<{ value: unknown; typ: Typ; name: string }> },
  ) => void;
  resetResult: () => void;
  exportName?: string;
}) => {
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [recursiveFormData, setRecursiveFormData] = useState<
    Record<string, unknown>
  >({});

  useEffect(() => {
    initialRecursiveFormData();
  }, [functionDetails]);

  const initialRecursiveFormData = () => {
    if (
      !functionDetails.parameters ||
      functionDetails.parameters.length === 0
    ) {
      setRecursiveFormData({});
      return;
    }

    const initialData = functionDetails.parameters.reduce(
      (acc, param) => {
        acc[param.name] = createEmptyValue(param.typ);
        return acc;
      },
      {} as Record<string, unknown>,
    );
    setRecursiveFormData(initialData);
  };

  const createEmptyValue = (typeDef: Typ): unknown => {
    const typeStr = typeDef.type?.toLowerCase();
    switch (typeStr) {
      case "record": {
        const record: Record<string, unknown> = {};
        typeDef.fields?.forEach((field: Field) => {
          record[field.name] = createEmptyValue(field.typ);
        });
        return record;
      }
      case "list":
        return [];
      case "option":
        return null;
      case "variant":
        // For variants, create the first case as default
        if (typeDef.cases && typeDef.cases.length > 0) {
          const firstCase = typeDef.cases[0];
          if (!firstCase) return null;
          if (typeof firstCase === "string") {
            // Unit variant - just return the case name
            return firstCase;
          } else {
            // Variant with data - create object with case name as key
            return { [firstCase.name]: createEmptyValue(firstCase.typ) };
          }
        }
        return null;
      case "str":
      case "chr":
        return "";
      case "bool":
        return false;
      case "enum":
        if (typeDef.cases && typeDef.cases.length > 0) {
          return typeDef.cases[0];
        }
        return "";
      case "flags":
        return [];
      case "result":
        // For results, create default ok value
        return { ok: typeDef.ok ? createEmptyValue(typeDef.ok) : null };
      case "tuple":
        // For tuples, create array with empty values for each element
        if (typeDef.fields && typeDef.fields.length > 0) {
          return typeDef.fields.map((field: Field) =>
            createEmptyValue(field.typ),
          );
        }
        return [];
      case "f64":
      case "f32":
      case "u64":
      case "s64":
      case "u32":
      case "s32":
      case "u16":
      case "s16":
      case "u8":
      case "s8":
        return 0;
      default:
        return null;
    }
  };

  const handleRecursiveParameterChange = (path: string, value: unknown) => {
    const updateNestedValue = (
      obj: Record<string, unknown>,
      pathArray: string[],
      value: unknown,
    ): Record<string, unknown> => {
      const [current, ...rest] = pathArray;
      if (!current) return obj;
      if (rest.length === 0) {
        return { ...obj, [current]: value };
      }
      return {
        ...obj,
        [current]: updateNestedValue(
          (obj[current] as Record<string, unknown>) || {},
          rest,
          value,
        ),
      };
    };

    setRecursiveFormData(prev =>
      updateNestedValue(prev, path.split("."), value),
    );
    resetResult();
  };

  const handleSubmit = () => {
    // Check if HTTP handler can be invoked directly
    const canInvoke = canInvokeHttpHandler(exportName);

    if (!canInvoke) {
      setErrors({
        root: "This HTTP handler cannot be invoked directly via CLI.",
      });
      return;
    }

    // Use recursive form data with type information
    const result: {
      params: Array<{ value: unknown; typ: Typ; name: string }>;
    } = { params: [] };
    if (functionDetails.parameters) {
      functionDetails.parameters.forEach(param => {
        const value = recursiveFormData[param.name];
        result.params.push({
          value,
          typ: param.typ,
          name: param.name,
        });
      });
    }
    onInvoke(result);
  };

  return (
    <div>
      <Card className="w-full">
        <form>
          <CardContent className="p-6">
            {/* Warning for HTTP handlers */}
            {!canInvokeHttpHandler(exportName) && (
              <div className="mb-6 p-4 bg-yellow-50 dark:bg-yellow-900/20 border border-yellow-200 dark:border-yellow-800 rounded-lg">
                <div className="flex items-start">
                  <Info className="w-5 h-5 text-yellow-600 dark:text-yellow-400 mt-0.5 mr-3 flex-shrink-0" />
                  <div>
                    <h4 className="text-sm font-medium text-yellow-800 dark:text-yellow-200">
                      Cannot invoke HTTP handler directly
                    </h4>
                    <p className="text-sm text-yellow-700 dark:text-yellow-300 mt-1">
                      This is an HTTP incoming handler that is designed to be
                      triggered by incoming HTTP requests, not direct CLI
                      invocation.
                    </p>
                  </div>
                </div>
              </div>
            )}

            {functionDetails.parameters &&
            functionDetails.parameters.length > 0 ? (
              // Recursive form layout
              <div className="space-y-4">
                {functionDetails.parameters.map(parameter => (
                  <RecursiveParameterInput
                    key={parameter.name}
                    name={parameter.name}
                    typeDef={parameter.typ}
                    value={recursiveFormData[parameter.name]}
                    onChange={handleRecursiveParameterChange}
                  />
                ))}
              </div>
            ) : (
              <div className="flex flex-col items-center justify-center text-center gap-4">
                <div>
                  <CircleSlash2 className="h-12 w-12 text-muted-foreground" />
                </div>
                <div>No Parameters</div>
                <div className="text-muted-foreground">
                  This function has no parameters. You can invoke it without any
                  arguments.
                </div>
              </div>
            )}

            {/* Display root errors */}
            {errors.root && (
              <div className="mt-4 p-3 bg-red-50 dark:bg-red-900/20 border border-red-200 dark:border-red-800 rounded-lg">
                <p className="text-sm text-red-700 dark:text-red-300">
                  {errors.root}
                </p>
              </div>
            )}
          </CardContent>
        </form>
      </Card>
      <div className="flex gap-4 justify-end mt-4">
        <Button
          variant="outline"
          onClick={initialRecursiveFormData}
          className="text-primary hover:bg-primary/10 hover:text-primary"
        >
          <TimerReset className="h-4 w-4 mr-1" />
          Reset
        </Button>
        <Button onClick={handleSubmit}>
          <Play className="h-4 w-4 mr-1" />
          Invoke
        </Button>
      </div>
    </div>
  );
};
