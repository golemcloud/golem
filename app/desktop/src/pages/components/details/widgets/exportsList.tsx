import { useMemo } from "react";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Globe } from "lucide-react";
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command";
import { ComponentExportFunction, Export } from "@/types/component";
import ErrorBoundary from "@/components/errorBoundary";

export function ExportsList({ exports }: { exports: Export[] }) {
  const exportedFunctions = useMemo(
    () =>
      exports.flatMap((exp: Export) =>
        exp.functions.map((func: ComponentExportFunction) => ({
          ...func,
          exportName: exp.name,
        })),
      ),
    [exports],
  );

  return (
    <ErrorBoundary>
      <Card>
        <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
          <CardTitle className="text-base font-medium">Exports</CardTitle>
          <Globe className="h-4 w-4 text-muted-foreground" />
        </CardHeader>
        <CardContent className="pt-4">
          <Command className="rounded-lg border shadow-none">
            <CommandInput placeholder="Search exports..." />
            <CommandList>
              <CommandEmpty>No exports found.</CommandEmpty>
              <CommandGroup>
                {exportedFunctions.map(endpoint => (
                  <CommandItem
                    key={`${endpoint.exportName}-${endpoint.name}`}
                    className="flex items-center justify-between"
                  >
                    <span className="font-mono text-sm">
                      <span className="text-blue-400">
                        {endpoint.exportName}.{endpoint.name}
                      </span>
                    </span>
                  </CommandItem>
                ))}
              </CommandGroup>
            </CommandList>
          </Command>
        </CardContent>
      </Card>
    </ErrorBoundary>
  );
}
