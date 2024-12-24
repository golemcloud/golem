"use client";   
import React, { useMemo } from "react";
import GenericTable from "@/components/ui/generic-table";
import useComponents from "@/lib/hooks/use-component";
import { ComponentExport, WorkerFunction } from "@/types/api";
import { useParams } from "next/navigation";
type Column<T> = {
    key: string;
    label: string;
    accessor: (item: T) => React.ReactNode;
    align?: "left" | "center" | "right";
    };
type ExportsTableProps<T> = {
    data: T[];
    columns: Column<T>[];
    onRowClick?: (item: T) => void;
    };

    export default function ExportsTable<T>({
        data,
        columns,
        onRowClick,
      }: ExportsTableProps<T>) {
        const { compId } = useParams<{ compId: string }>();
        const { components, isLoading: componentDataLoading } = useComponents(
          compId,
          "latest"
        );
        const [latestComponent] = components;
      
    
        const exports = useMemo(() => {
          const metaExports = (latestComponent?.metadata?.exports || []) as ComponentExport[];
          
          return metaExports.flatMap((exportItem) =>
            exportItem.functions.map((func) => ({
              name: exportItem.name,
              functionName: func.name,
              paramters: func.parameters,
              results: func.results,
            }))
          );
        }, [latestComponent?.versionedComponentId?.version]);
      
        console.log(exports);
      
        return (
          <GenericTable
            data={exports}
            columns={[
              {
                key: "package",
                label: "Package",
                accessor: (item: any) => item.name,
              },
              {
                key: "function",
                label: "Function",
                accessor: (item: any) => item.functionName,
              },
              {
                key: "parameters",
                label: "Parameters",
                accessor: (item: any) => item.paramters,
              },
              {
                key: "return",
                label: "Return value",
                accessor: (item: any) => item.results,
              },
            ]}
            onRowClick={()=>{}}
          />
        );
      }
      