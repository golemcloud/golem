import React from 'react';
import { Search } from 'lucide-react';
import { Input } from "@/components/ui/input";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import ComponentLeftNav from './componentsLeftNav';


interface ApiFunction {
  package: string
  function: string
  parameters: string
  returnValue: string
}

const apiFunctions: ApiFunction[] = [
  {
    package: "golem:component/api",
    function: "initialize-cart",
    parameters: "( user-id: string )",
    returnValue: ""
  },
  {
    package: "golem:component/api",
    function: "add-item",
    parameters: "( item: record )",
    returnValue: ""
  },
  {
    package: "golem:component/api",
    function: "remove-item",
    parameters: "( product-id: string )",
    returnValue: ""
  },
  {
    package: "golem:component/api",
    function: "update-item-quantity",
    parameters: "( product-id: string, quantity: u32 )",
    returnValue: ""
  },
  {
    package: "golem:component/api",
    function: "checkout",
    parameters: "",
    returnValue: "variant { error(string), success(record { order-id: string }) }"
  },
  {
    package: "golem:component/api",
    function: "get-cart-contents",
    parameters: "",
    returnValue: "list<record>"
  }
]

export default function Exports() {
  return (
    <div className="flex">
    <ComponentLeftNav />
    <div className="p-6 max-w-7xl mx-auto space-y-6">
    <div className="flex justify-between items-center">
      <h1 className="text-2xl font-bold">Exports</h1>
    </div>
      <div className="flex items-center justify-between">
        <div className="relative flex-1 max-w-xl">
          <Search className="absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
          <Input 
            placeholder="Search functions..." 
            className="pl-9"
          />
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
            {apiFunctions.map((fn, index) => (
              <TableRow key={index}>
                <TableCell className="font-mono text-sm">
                  {fn.package}
                </TableCell>
                <TableCell className="font-mono text-sm">
                  {fn.function}
                </TableCell>
                <TableCell className="font-mono text-sm">
                  {fn.parameters}
                </TableCell>
                <TableCell className="font-mono text-sm">
                  {fn.returnValue}
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </div>
    </div>
    </div>
  )
}





