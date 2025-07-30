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
import ErrorBoundary from "@/components/errorBoundary";

export function ExportsList({ exports }: { exports: string[] }) {
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
                {exports.map((endpoint: string, index) => (
                  <CommandItem
                    key={`${endpoint}-${index}`}
                    className="flex items-center justify-between"
                  >
                    <span className="font-mono text-sm">
                      <span className="text-blue-400">{endpoint}</span>
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
