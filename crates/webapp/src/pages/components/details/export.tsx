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
import { Export, Field, Parameter, Result, Typ } from "@/types/component.ts";
import ErrorBoundary from "@/components/errorBoundary";

// Recursive utility function to parse JSON and identify patterns
function parseType(typ: Typ) {
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
  const { componentId } = useParams();
  const [exports, setExports] = useState({} as Export);
  useEffect(() => {
    API.getComponentById(componentId!).then((res) => {
      if (res.metadata!.exports && res.metadata!.exports.length > 0) {
        setExports(res.metadata!.exports[0]);
      }
    });
  }, [componentId]);

  return (
    <ErrorBoundary>
      <div className="flex">
        <ComponentLeftNav />
        <div className="flex-1 flex flex-col">
          <header className="w-full border-b bg-background py-4">
            <div className="mx-auto px-6 lg:px-8">
              <div className="flex items-center gap-4">
                <h1 className="text-xl font-semibold text-foreground truncate">
                  {componentId}
                </h1>
              </div>
            </div>
          </header>
          <div className="flex-1 p-8">
            <div className="p-6 max-w-7xl mx-auto space-y-6">
              <div className="flex justify-between items-center">
                <h1 className="text-2xl font-bold">Exports</h1>
              </div>
              <div className="flex items-center justify-between">
                <div className="relative flex-1 max-w-xl">
                  <Search className="absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
                  <Input placeholder="Search functions..." className="pl-9" />
                </div>
                <Select defaultValue="v0">
                  <SelectTrigger className="w-24">
                    <SelectValue placeholder="Version" />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="v0">v0</SelectItem>
                    <SelectItem value="v1">v1</SelectItem>
                    <SelectItem value="v2">v2</SelectItem>
                  </SelectContent>
                </Select>
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
                    {exports.functions &&
                      exports.functions.map((fn, index) => (
                        <TableRow key={index}>
                          <TableCell className="font-mono text-sm">
                            {exports.name}
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
                      ))}
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
