import { Search } from "lucide-react";
import { Input } from "@/components/ui/input";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import ComponentLeftNav from "./componentsLeftNav";
import { useEffect, useState } from "react";
import { API } from "@/service";
import { useParams } from "react-router-dom";
import {
  Component,
  ComponentExportFunction,
  Export,
  Field,
  Parameter,
  Result,
  Typ,
} from "@/types/component.ts";
import ErrorBoundary from "@/components/errorBoundary";
import { calculateExportFunctions } from "@/lib/utils";

function parseType(typ: Typ): string {
  if (typ.type) {
    if (typ.type === "Record" && typ.fields) {
      return `{
        ${typ.fields
          .map((field: Field) => `${field.name}: ${parseType(field.typ)}`)
          .join(", \n        ")}
      }`;
    } else if (typ.type === "Array" && typ.fields) {
      return `Array<{
        ${typ.fields
          .map((field: Field) => `${field.name}: ${parseType(field.typ)}`)
          .join(", \n        ")}
      }>`;
    } else if (typ.type === "Variant" && typ.cases) {
      return typ.cases
        .map((caseItem) => {
          if (caseItem.typ.fields) {
            return `${caseItem.name}: {
              ${caseItem.typ.fields
                .map((field) => `${field.name}: ${parseType(field.typ)}`)
                .join(", \n              ")}
            }`;
          }
          return `${caseItem.name}: ${parseType(caseItem.typ)}`;
        })
        .join(" | ");
    }
    return typ.type;
  }
  return "Unknown";
}

// Main function to convert JSON to function structure
function convertJsonToFunctionStructure(json: Parameter[] | Result[]) {
  return json.map((entry) => {
    const name = entry.name;
    const fieldsStructure = parseType(entry.typ);
    return `(${name}: ${fieldsStructure})`;
  });
}

export default function Exports() {
  const { componentId = "" } = useParams();
  const [componentList, setComponentList] = useState([] as Component[]);
  const [component, setComponent] = useState<Component>({});
  const [versionList, setVersionList] = useState([] as number[]);
  const [versionChange, setVersionChange] = useState(0 as number);
  const [functions, setFunctions] = useState([] as ComponentExportFunction[]);

  useEffect(() => {
    if (componentId) {
      API.getComponents().then((response) => {
        setComponentList(response);
      });

      API.getComponentByIdAsKey().then((response) => {
        setVersionList(response[componentId].versionId || []);
        setComponent(response[componentId]);
      });
    }
  }, [componentId]);

  useEffect(() => {
    if (component && component.exports) {
      const functions = component.exports.reduce(
        (acc: ComponentExportFunction[], curr: Export) => {
          const updatedFunctions = curr.functions.map(
            (func: ComponentExportFunction) => ({
              ...func,
              exportName: curr.name,
            })
          );

          return acc.concat(updatedFunctions);
        },
        []
      );
      setFunctions(functions);
    }
  }, [component]);

  const handleVersionChange = (version: number) => {
    setVersionChange(version);
    const componentDetails = componentList.find((component: Component) => {
      if (component.versionedComponentId) {
        return (
          component.versionedComponentId.componentId === componentId &&
          component.versionedComponentId.version === version
        );
      }
    });
    if (componentDetails) {
      setComponent({
        ...componentDetails,
        exports: componentDetails.metadata?.exports,
      });
    } else {
      setComponent({});
    }
  };

  const handleSearch = (e: React.ChangeEvent<HTMLInputElement>) => {
    const value = e.target.value;

    const searchResult = calculateExportFunctions(
      component.exports || []
    ).filter((fn: ComponentExportFunction) => {
      return fn.name.includes(value);
    });
    setFunctions(searchResult || ([] as ComponentExportFunction[]));
  };

  return (
    <ErrorBoundary>
      <div className="flex">
        <ComponentLeftNav componentDetails={component} />
        <div className="flex-1 flex flex-col">
          <header className="w-full border-b bg-background py-4">
            <div className="mx-auto px-6 lg:px-8">
              <div className="flex items-center gap-4">
                <h1 className="text-xl font-semibold text-foreground truncate">
                  {component.componentName}
                </h1>
              </div>
            </div>
          </header>
          <div className="flex-1 p-8">
            <div className="p-6 max-w-7xl mx-auto space-y-6">
              <div className="flex justify-between items-center">
                <h1 className="text-2xl font-bold">Exports</h1>
              </div>
              <div className="flex items-center justify-between gap-10">
                <div className="relative flex-1 max-full">
                  <Search className="absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
                  <Input
                    placeholder="Search functions..."
                    className="pl-9"
                    onChange={(e) => handleSearch(e)}
                  />
                </div>
                {versionList.length > 0 && (
                  <Select
                    defaultValue={versionChange.toString()}
                    onValueChange={(version) => handleVersionChange(+version)}
                  >
                    <SelectTrigger className="w-[80px]">
                      <SelectValue> v{versionChange} </SelectValue>
                    </SelectTrigger>
                    <SelectContent>
                      {versionList.map((version: number) => (
                        <SelectItem key={version} value={String(version)}>
                          v{version}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                )}
              </div>

              <div className="border rounded-lg">
                <Table>
                  <TableHeader>
                    <TableRow>
                      <TableHead className="w-[250px]">Package</TableHead>
                      <TableHead className="w-[200px]">Function</TableHead>
                      <TableHead className="w-[300px]">Parameters</TableHead>
                      <TableHead>Return Value</TableHead>
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    {functions?.length > 0 ? (
                      functions.map((fn: ComponentExportFunction) => (
                        <TableRow key={fn.name}>
                          <TableCell className="font-mono text-sm">
                            {fn.exportName}
                          </TableCell>
                          <TableCell className="font-mono text-sm">
                            {fn.name}
                          </TableCell>
                          <TableCell className="font-mono text-sm">
                            {convertJsonToFunctionStructure(fn.parameters)}
                          </TableCell>
                          <TableCell className="font-mono text-sm">
                            {convertJsonToFunctionStructure(fn.results)}
                          </TableCell>
                        </TableRow>
                      ))
                    ) : (
                      <div className="p-4 align-center grid">
                        No exports found.
                      </div>
                    )}
                  </TableBody>
                </Table>
              </div>
            </div>
          </div>
        </div>
      </div>
    </ErrorBoundary>
  );
}
