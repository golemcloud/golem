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
import { useEffect, useState } from "react";
import { API } from "@/service";
import { useParams } from "react-router-dom";
import {
  ComponentExportFunction,
  ComponentList,
  Export,
  Typ,
} from "@/types/component";
import { calculateExportFunctions } from "@/lib/utils";

/**
 * Shape of the final interface for each export/function.
 */
export interface ExportResult {
  package: string;
  function_name: string;
  parameter: string;
  return: string;
}

/**
 * Renders a type name in HTML using inline styles/classes for color.
 */
function parseTypeV1(typ: Typ): string {
  if (!typ) return "null";

  if (typ.type === "Str") return `<span class="text-purple-800">string</span>`;
  if (typ.type === "U64") return `<span class="text-purple-800">number</span>`;
  if (typ.type === "Bool")
    return `<span class="text-purple-800">boolean</span>`;
  if (typ.type === "Option") {
    return `<span class="text-purple-800">${parseTypeV1(
      typ.inner!
    )}</span> <span className="text-green-700">or</span> null`;
  }
  if (typ.type === "List") return `${parseTypeV1(typ.inner!)}[]`;

  if (typ.type === "Record") {
    const fields = (typ.fields || []).map(
      (field) =>
        `<span class="text-yellow-600">${field.name}</span>: ${parseTypeV1(
          field.typ
        )}`
    );
    return `{ ${fields.join(", ")} }`;
  }

  if (typ.type === "Result") {
    return `${parseTypeV1(
      typ.ok!
    )}  <span class="text-green-700">or</span>  ${parseEnumV1(typ.err!)}`;
  }

  return "unknown";
}

function parseEnumV1(enumType: Typ): string {
  if (!enumType || enumType.type !== "Enum") return "unknown";
  return enumType.cases!.map((c) => `'${c}'`).join(" | ");
}

/**
 * Generates a list of functions (with typed parameters and returns) from an array of exports,
 * returning them in a more readable format (HTML strings for coloring).
 */
function generateFunctionInterfacesV1(data: Export[]): ExportResult[] {
  const interfaces: ExportResult[] = [];

  data.forEach((instance) => {
    instance.functions.forEach((func) => {
      // Convert kebab-case to camelCase
      const functionName = func.name.replace(/-([a-z])/g, (_, letter: string) =>
        letter.toUpperCase()
      );

      const parameters = func.parameters
        .map(
          (param) =>
            `<span class="text-yellow-600">${param.name}</span>: ${parseTypeV1(
              param.typ
            )}`
        )
        .join(", <br/> ");

      const returnType = parseTypeV1(func.results[0]?.typ) || "void";

      interfaces.push({
        package: instance.name,
        function_name: functionName,
        parameter: parameters,
        return: returnType,
      });
    });
  });

  return interfaces;
}

export default function Exports() {
  /**
   * The componentId is extracted from the URL route parameters.
   * If not provided, defaults to an empty string.
   */
  const { componentId = "" } = useParams();

  /**
   * Local state for the selected component data, version list,
   * currently selected version, and the final export results to display.
   */
  const [component, setComponent] = useState<ComponentList>({});
  const [versionList, setVersionList] = useState<number[]>([]);
  const [versionChange, setVersionChange] = useState<number>(0);
  const [result, setResult] = useState<ExportResult[]>([]);

  /**
   * Additional state for searching among function exports.
   * Currently, we only use this to filter out matches in `calculateExportFunctions`.
   * If we decide to filter `result` as well, that logic can be integrated here.
   */
  const [functions, setFunctions] = useState<ComponentExportFunction[]>([]);

  useEffect(() => {
    if (!componentId) return;

    // 1) Fetch entire list of components by ID, then set local states (component, versionList, versionChange)
    API.getComponentByIdAsKey().then((response) => {
      const fetched = response[componentId];
      if (!fetched) return;

      const versions = fetched.versionList || [];
      setVersionList(versions);

      // Default to the latest version
      const selectedVersion = versions[versions.length - 1] || 0;
      setVersionChange(selectedVersion);

      setComponent(fetched);
    });
  }, [componentId]);

  /**
   * Effect that reacts to any changes in the local `component` or `versionChange`.
   * Once both are set, we find the details for the selected version
   * and generate a list of export results.
   */
  useEffect(() => {
    if (!component.versions?.length || versionChange === 0) return;

    const componentDetails = component.versions.find(
      (data) => data.versionedComponentId?.version === versionChange
    );
    if (!componentDetails) {
      setResult([]);
      return;
    }

    // Convert exports to the final interface format
    const exportsResult: ExportResult[] = generateFunctionInterfacesV1(
      componentDetails.metadata?.exports || []
    );
    setResult(exportsResult);

    // (Optional) If you want to maintain a separate array of raw functions, you can do so here.
    // This is presumably for a different display or search usage.
    const rawFunctions: ComponentExportFunction[] = calculateExportFunctions(
      componentDetails.metadata?.exports || []
    );
    setFunctions(rawFunctions);
  }, [component, versionChange]);

  /**
   * Handle version changes in the select dropdown
   */
  const handleVersionChange = (version: number) => {
    setVersionChange(version);
  };

  /**
   * Search among the raw function list using `calculateExportFunctions`.
   * If you prefer to filter the `result` array itself, adapt this logic accordingly.
   */
  const handleSearch = (e: React.ChangeEvent<HTMLInputElement>) => {
    const value = e.target.value.toLowerCase();

    // Filter the full function list
    const searchResult = calculateExportFunctions(
      component.versions?.find(
        (data) => data.versionedComponentId?.version === versionChange
      )?.metadata?.exports || []
    ).filter((fn: ComponentExportFunction) =>
      fn.name.toLowerCase().includes(value)
    );

    setFunctions(searchResult);
    // If you want the table to reflect the filtered results,
    // you could also parse them into the same `result` format here.
  };

  return (
    <div className="flex">
      <div className="flex-1 p-8">
        <div className="p-6 max-w-7xl mx-auto space-y-6">
          {/* Header */}
          <div className="flex justify-between items-center">
            <h1 className="text-2xl font-bold">Exports</h1>
          </div>

          {/* Search + Version Select */}
          <div className="flex items-center justify-between gap-10">
            <div className="relative flex-1 max-full">
              <Search className="absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
              <Input
                placeholder="Search functions..."
                className="pl-9"
                onChange={(e) => handleSearch(e)}
              />
            </div>
            {/* Version selector if there are available versions */}
            {versionList.length > 0 && (
              <Select
                defaultValue={versionChange.toString()}
                onValueChange={(version) => handleVersionChange(+version)}
              >
                <SelectTrigger className="w-[80px]">
                  <SelectValue>v{versionChange}</SelectValue>
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

          {/* Table of Exported Functions */}
          <div className="border rounded-lg">
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead className="w-[250px]">Package</TableHead>
                  <TableHead className="w-[200px]">Function</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {result?.length > 0 ? (
                  result.map((fn: ExportResult) => (
                    <TableRow
                      key={`${fn.package}-${fn.function_name}`}
                      /* Combined key to reduce chance of collision */
                    >
                      <TableCell className="font-mono text-sm">
                        {fn.package}
                      </TableCell>
                      <TableCell className="font-mono text-sm">
                        <span>{fn.function_name}</span>(
                        <span
                          dangerouslySetInnerHTML={{ __html: fn.parameter }}
                        />
                        ) {"=>"}{" "}
                        <span dangerouslySetInnerHTML={{ __html: fn.return }} />
                      </TableCell>
                    </TableRow>
                  ))
                ) : (
                  // No exports found for this version
                  <div className="p-4 align-center grid">No exports found.</div>
                )}
              </TableBody>
            </Table>
          </div>
        </div>
      </div>
    </div>
  );
}
