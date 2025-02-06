/* eslint-disable @typescript-eslint/no-explicit-any */
"use client";
import React, { useMemo } from "react";
import GenericTable from "@/components/ui/generic-table";
import useComponents from "@/lib/hooks/use-component";
import { ComponentExport } from "@/types/api";
import SecondaryHeader from "@/components/ui/secondary-header";
import { Box } from "@mui/material";
import ErrorBoundary from "@/components/error-boundary";
import { useCustomParam } from "@/lib/hooks/use-custom-param";

type DataItem = {
  package: string;
  method: string;
  parameters: string;
  returnType: string;
};

export default function ExportsPage() {
  const { compId } = useCustomParam();
  const { components, error } = useComponents(compId, "latest");
  const [latestComponent] = components;

  const exports = useMemo(() => {
    const metaExports = (latestComponent?.metadata?.exports || []) as ComponentExport[];
    return metaExports.flatMap((exportItem) =>
      exportItem.type === "Instance"
        ? exportItem.functions.map((func) => ({
            name: exportItem.name,
            functionName: func.name,
            parameters: func.parameters,
            results: func.results,
          }))
        : {
            name: "",
            functionName: exportItem.name,
            parameters: exportItem.parameters,
            results: exportItem.results,
          }
    );
  }, [latestComponent?.versionedComponentId?.version]);

  const tableData: DataItem[] = exports.map((func) => ({
    package: func.name,
    method: func.functionName,
    parameters: func.parameters
      .map((param) => `${param.name}: ${param?.typ?.type}`)
      .join(", "),
    returnType: func.results
      .map((result) => {
        if (result?.typ?.type === "Variant") {
          return result?.typ?.cases
            ?.map((variant) => `${variant.name}(${variant?.typ?.type || "Record"})`)
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
      {error && <ErrorBoundary message={error}/>}
      <div className="mx-auto max-w-7xl px-2 md:px-6 lg:px-8">

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
                  item.parameters
                    .split(", ")
                    .map((param: string, index: number) => (
                      <div key={index}>
                        <strong>{param.split(":")[0]}:</strong> {param.split(":")[1]}
                      </div>
                    )),
              },
              {
                key: "results",
                label: "Return Type",
                accessor: (item: any) =>
                  item.returnType
                    .split(", ")
                    .map((result: string, index: number) => (
                      <div key={index}>
                        <strong>{result.split(":")[0]}:</strong> {result.split(":")[1]}
                      </div>
                    )),
              },
            ]}
            // onRowClick={onRowClick}
          />
        </div>
      </div>
    </>
  );
}
