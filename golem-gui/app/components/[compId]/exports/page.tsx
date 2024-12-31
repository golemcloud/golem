"use client";
import React, { useMemo } from "react";
import GenericTable from "@/components/ui/generic-table";
import useComponents from "@/lib/hooks/use-component";
import { ComponentExport } from "@/types/api";
import { useParams } from "next/navigation";
import SecondaryHeader from "@/components/ui/secondary-header";
import { Box } from "@mui/material";

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

type DataItem = {
  package: string;
  method: string;
  parameters: string;
  returnType: string;
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
     exportItem.type==="Instance" ? exportItem.functions.map((func) => ({
        name: exportItem.name,
        functionName: func.name,
        parameters: func.parameters,
        results: func.results,
      })) : {
        name: "",
        functionName: exportItem.name,
        parameters: exportItem.parameters,
        results: exportItem.results,
      }
    );
  }, [latestComponent?.versionedComponentId?.version]);

  console.log(exports);
  const tableData: DataItem[] = exports.map((func) => ({
    package: func.name,
    method: func.functionName,
    parameters: func.parameters
      .map(
        (param) =>
          `${param.name}: ${
            param?.typ?.type 
          }`
      )
      .join(", "),
    returnType: func.results
      .map((result) => {
        if (result?.typ?.type === "Variant") {
          return result?.typ?.cases?.map(
              (variant) =>
                `${variant.name}(${variant?.typ?.type || "Record"})`
            )
            .join(" | ");
        } else if (result?.typ?.type === "List") {
          return `List<Record>`;
        } else {
          return result?.typ?.type || "None";
        }
      })
      .join(", "),
  }));

  return (
    <>
    <Box sx={{ display: { xs: "block", md: "none" } }}>
        <SecondaryHeader onClick={() => {}} variant="components" />
      </Box>
    <div className="mx-auto max-w-7xl px-6 lg:px-8">
      <div className="mx-auto max-w-2xl lg:max-w-none py-4">
    
    <GenericTable
      data={tableData}
      columns={[
        {
          key: "package",
          label: "Package",
          accessor: (item: any) => item.package,
        },
        {
          key: "function",
          label: "Function",
          accessor: (item: any) => item.method,
        },
        {
          key: "parameters",
          label: "Parameters",
          accessor: (item: any) =>
            item.parameters.split(", ").map((param: string, index: number) => (
              <div key={index}>
                <strong>{param.split(":")[0]}:</strong> {param.split(":")[1]}
              </div>
            )),
        },
        {
          key: "results",
          label: "Return Type",
          accessor: (item: any) =>
            item.returnType.split(", ").map((result: string, index: number) => (
              <div key={index}>
                <strong>{result.split(":")[0]}:</strong> {result.split(":")[1]}
              </div>
            )),
        },
      ]}
    //   onRowClick={onRowClick}
    />
    </div>
    </div>
    </>
  );
}
