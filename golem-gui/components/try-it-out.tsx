"use client";
import DynamicForm from "@/app/components/[compId]/workers/[id]/form-generator";
import { useCustomParam } from "@/lib/hooks/use-custom-param";
import { ApiDeployment, ApiRoute, Parameter } from "@/types/api";
import { AnalysedType_TypeRecord } from "@/types/golem-data-types";
import { Box, MenuItem, Select, Stack, Typography } from "@mui/material";
import React, { useMemo, useState } from "react";
import useApiDeployments from "@/lib/hooks/use-api-deployments";
import JsonEditor from "./json-editor";
import { Loader } from "lucide-react";
import ErrorBoundary from "./erro-boundary";
import { toast } from "react-toastify";
type FormData = {
  [key: string]: unknown;
};
export default function TryItOut({
  route,
  version,
}: {
  route: ApiRoute;
  version: string;
}) {
  const { apiId } = useCustomParam();
  const [deployment, setDeployment] = useState<ApiDeployment | null>();
  const [curl, setCurl] = useState<string | null>(null);
  const { apiDeployments, isLoading, error } = useApiDeployments(apiId);
  const finalDeployments = apiDeployments?.filter((deploy) =>
    deploy?.apiDefinitions?.find(
      (api) => api.id === apiId && api.version === version
    )
  );
  const routeMeta = useMemo(() => {
    if (!route) {
      return null;
    }
    const meta = {
      name: "request",
      typ: {
        fields: [] as Parameter[],
        type: "Record",
      } as AnalysedType_TypeRecord,
    } as Parameter;

    const responseInput = route?.binding?.responseMappingInput?.types?.request;

    if (responseInput && meta?.typ?.type === "Record" && meta?.typ?.fields) {
      const bodyFields =
        (responseInput.type === "Record" &&
          responseInput.fields?.filter((field) => field.name === "body")) ||
        [];

      if (bodyFields.length > 0) {
        meta.typ.fields.push(bodyFields[0]);
      } else {
        meta.typ.fields.push({
          name: "body",
          typ: {
            type: "Record",
            fields: [],
          } as AnalysedType_TypeRecord,
        });
      }

      const pathFields =
        (responseInput.type === "Record" &&
          responseInput.fields?.filter((field) => field.name === "path")) ||
        [];

      if (pathFields.length > 0) {
        meta.typ.fields.push(pathFields[0]);
      } else {
        meta.typ.fields.push({
          name: "path",
          typ: {
            type: "Record",
            fields: [],
          } as AnalysedType_TypeRecord,
        });
      }
    }

    const workerInput = route?.binding?.workerNameInput?.types?.request;
    if (workerInput && meta?.typ?.type === "Record" && meta?.typ?.fields) {
      const bodyFields =
        (workerInput.type === "Record" &&
          workerInput.fields?.filter((field) => field.name === "body")) ||
        [];
      if (bodyFields.length > 0) {
        meta.typ.fields.push(bodyFields[0]);
      } else {
        meta.typ.fields.push({
          name: "body",
          typ: {
            type: "Record",
            fields: [],
          } as AnalysedType_TypeRecord,
        });
      }

      const pathFields =
        (workerInput.type === "Record" &&
          workerInput.fields?.filter((field) => field.name === "path")) ||
        [];

      if (pathFields.length > 0) {
        meta.typ.fields.push(pathFields[0]);
      } else {
        meta.typ.fields.push({
          name: "path",
          typ: {
            type: "Record",
            fields: [],
          } as AnalysedType_TypeRecord,
        });
      }
    }

    return [meta];
  }, [route]);

  async function copyCurlToClipboard(
    method: string,
    url: string,
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    body: any = null
  ) {
    let curlCommand = `curl -X ${method.toUpperCase()} "${url}"`;

    // Add common headers
    curlCommand += ` -H "accept: text/html,application/json,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/apng,*/*;q=0.8,application/signed-exchange;v=b3;q=0.7"`;
    curlCommand += ` -H "accept-language: en-US,en;q=0.9"`;
    curlCommand += ` -H "cache-control: max-age=0"`;

    // If the request is POST, PUT, or PATCH, include the Content-Type header
    if (
      method.toUpperCase() === "POST" ||
      method.toUpperCase() === "PUT" ||
      method.toUpperCase() === "PATCH"
    ) {
      curlCommand += ` -H "Content-Type: application/json"`;
    }

    // If there is a body, add the data using the -d flag
    if (body) {
      curlCommand += ` -d '${JSON.stringify(body)}'`;
    }

    // Copy the generated curl command to the clipboard
    await navigator.clipboard.writeText(curlCommand);

    return curlCommand;
  }

  const transformPath = (path: string, pathParams: Record<string, string>): string => {
    return path.replace(/{([^}]+)}/g, (_, key) => pathParams[key] || `{${key}}`);
  };
  const handleSubmit = async (data: FormData) => {
    if (!deployment) {
      return alert('Select deployment');
    }

    try{
      const tansformedPath = transformPath(route.path, data?.request?.path)
      const curl = await copyCurlToClipboard(
        route.method,
        `https://${deployment.site.subdomain}.${deployment.site.host}${tansformedPath}`,
        data?.request?.body
      );
      setCurl(curl);
      toast.success("Curl request has been copied to the clipboard.")
    }catch(err){
      console.error("somthing went wrong!", err);
      toast.error("Something went wrong while creating curl");
    } 
    
  };

  if (isLoading) {
    return <Loader />;
  }

  if (error) {
    return <ErrorBoundary message={error} />;
  }

  return (
    <Box className="w-full">
      {route && routeMeta ? (
        <Box>
          <Stack direction="row" justifyContent={"center"} my={2}>
            <Typography color="red">Experimental</Typography>
          </Stack>

          <DynamicForm config={routeMeta} onSubmit={handleSubmit}  invokeLabel={"Copy Curl"}/>
          <Typography>Select Host</Typography>
          <Select
            variant="outlined"
            className="w-[250px] mb-3"
            required
            defaultValue={finalDeployments[0]}
            onChange={(e) => {
              const selectedIndex = Number(e.target.value);
              if (selectedIndex < 0 || isNaN(selectedIndex)) {
                return;
              }
              setDeployment(finalDeployments[selectedIndex]);
            }}
          >
            {finalDeployments.map(
              (deployment: ApiDeployment, in_idx: number) => (
                <MenuItem
                  key={`${deployment.site.host}_${deployment.site.subdomain}`}
                  value={in_idx}
                >
                  {`${deployment.site.subdomain}.${deployment.site.host}`}
                </MenuItem>
              )
            )}
          </Select>
          
          {curl && <JsonEditor json={curl} />}
        </Box>
      ) : (
        <Typography>No Route Found!</Typography>
      )}
    </Box>
  );
}
