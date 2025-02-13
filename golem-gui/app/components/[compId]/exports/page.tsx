"use client";
import React, { useMemo } from "react";
import GenericTable from "@/components/ui/generic-table";
import useComponents from "@/lib/hooks/use-component";
import { ComponentExport } from "@/types/api";
import SecondaryHeader from "@/components/ui/secondary-header";
import { Box } from "@mui/material";
import ErrorBoundary from "@/components/error/error-boundary";
import { useCustomParam } from "@/lib/hooks/use-custom-param";
import { PopoverDemo } from "@/app/apis/interpolate-tooltip";
import { Button2 } from "@/components/imports";

type DataItem = {
  package: string;
  method: string;
  parameters: string;
  returnType: string;
};

const formatRecordType = (fields: any[]): string => {
  const formattedFields = fields
    .map((field) => `    ${field.name}: ${field.typ.type.toLowerCase()}`)
    .join(",\n");
  return `record {\n${formattedFields}\n  }`;
};

const formatVariantType = (result: any): string => {
  if (!result?.typ?.cases) return "variant {}";

  const cases = result.typ.cases
    .map((caseItem: any) => {
      const typeName = caseItem.typ.type.toLowerCase();
      if (typeName === "record" && caseItem.typ.fields) {
        return `  ${caseItem.name}(${formatRecordType(caseItem.typ.fields)})`;
      }
      return `  ${caseItem.name}(${typeName})`;
    })
    .join(",\n");

  return `variant {\n${cases}\n}`;
};

export default function ExportsPage() {
  const { compId } = useCustomParam();
  const { components, error } = useComponents(compId, "latest");
  const [latestComponent] = components;

  const exports = useMemo(() => {
    const metaExports = (latestComponent?.metadata?.exports ||
      []) as ComponentExport[];
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
      .map((param) => `${param.name}: ${param?.typ?.type?.toLowerCase()}`)
      .join(", "),
    returnType: func.results
      .map((result) => {
        if (result?.typ?.type === "Variant") {
          return formatVariantType(result);
        } else if (result?.typ?.type === "List") {
          return `list<record>`;
        } else {
          return result?.typ?.type?.toLowerCase() || "none";
        }
      })
      .join(", "),
  }));

  return (
    <>
      <Box sx={{ display: { xs: "block", md: "none" } }}>
        <SecondaryHeader onClick={() => {}} variant='components' />
      </Box>
      {error && <ErrorBoundary message={error} />}
      <div className='mx-auto max-w-7xl px-2 md:px-6 lg:px-8'>
        <div className='mx-auto max-w-2xl lg:max-w-none py-4'>
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
                accessor: (item: any) => (
                  <div className='flex gap-1 font-mono text-xs'>
                    {item.parameters ? <>({item.parameters})</> : ""}
                  </div>
                ),
              },
              {
                key: "results",
                label: "Return Type",
                accessor: (item: any) => (
                  <PopoverDemo
                    Icon={
                      item.returnType ? (
                        <Button2
                          variant='dropdown'
                          size='icon_sm'
                          className='font-mono text-xs'
                        >
                          variant
                        </Button2>
                      ) : (
                        <></>
                      )
                    }
                  >
                    <pre className='whitespace-pre font-mono text-sm text-muted-foreground'>
                      {item.returnType}
                    </pre>
                  </PopoverDemo>
                ),
              },
            ]}
          />
        </div>
      </div>
    </>
  );
}
