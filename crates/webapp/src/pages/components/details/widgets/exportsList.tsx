import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card"
import { Globe } from 'lucide-react'
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command"

const API_ENDPOINTS = [
  "golem:component/api.{initialize-cart}",
  "golem:component/api.{add-item}",
  "golem:component/api.{remove-item}",
  "golem:component/api.{update-item-quantity}",
  "golem:component/api.{checkout}",
  "golem:component/api.{get-cart-contents}"
]

export function ExportsList() {
  return (
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
              {API_ENDPOINTS.map((endpoint) => (
                <CommandItem
                  key={endpoint}
                  className="flex items-center justify-between"
                >
                  <span className="text-sm">{endpoint}</span>
                </CommandItem>
              ))}
            </CommandGroup>
          </CommandList>
        </Command>
      </CardContent>
    </Card>
  )
}

