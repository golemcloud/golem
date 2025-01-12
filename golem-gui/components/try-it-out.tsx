'use client'
import DynamicForm from "@/app/components/[compId]/workers/[id]/form-generator";
import { useCustomParam } from "@/lib/hooks/use-custom-param";
import { ApiDeployment, ApiRoute, Parameter } from "@/types/api";
import {
  AnalysedType_TypeRecord,
} from "@/types/golem-data-types";
import { MenuItem, Select, Typography } from "@mui/material";
import React, { useMemo, useState } from "react";
import useApiDeployments from "@/lib/hooks/use-api-deployments";
import JsonEditor from "./json-editor";
type FormData = {
  [key: string]: unknown;
};
export default function TryItOut({ route }: { route: ApiRoute }) {
  const {apiId} = useCustomParam();
  const [deployment, setDeployment] = useState<ApiDeployment|null>();
  const [curl, setCurl] = useState<string|null>(null)
  const {apiDeployments, isLoading, error} = useApiDeployments(apiId)
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
            fields: []
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
            fields: []
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
            fields: []
          } as AnalysedType_TypeRecord,
        });
      }
    }

    return meta;
  }, [route]);


  async function copyCurlToClipboard(method: string, url: string, body: any = null) {
    let curlCommand = `curl -X ${method.toUpperCase()} "${url}"`;
  
    // Add common headers
    curlCommand += ` -H "accept: text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/apng,*/*;q=0.8,application/signed-exchange;v=b3;q=0.7"`;
    curlCommand += ` -H "accept-language: en-US,en;q=0.9"`;
    curlCommand += ` -H "cache-control: max-age=0"`;
  
    // If the request is POST, PUT, or PATCH, include the Content-Type header
    if (method.toUpperCase() === 'POST' || method.toUpperCase() === 'PUT' || method.toUpperCase() === 'PATCH') {
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
  
  

  const handleSubmit = async (data: FormData) => {
    if(!deployment){
      return;
    }

    const curl = await copyCurlToClipboard(route.method, 
      `https://${deployment.site.subdomain}.${deployment.site.host}${route.path}`, data?.request?.body);
    setCurl(curl)

    
  };
  return (
    <div>
      {route && routeMeta ? (
        <>
                <DynamicForm config={[routeMeta]} onSubmit={handleSubmit} />
                <Select
                    variant="outlined"
                    className="max-w-max"
                    value={deployment}
                    onChange={(e) => {
                      const selectedIndex = Number(e.target.value);
                      if (selectedIndex < 0 || isNaN(selectedIndex)) {
                        return;
                      }
                      setDeployment(apiDeployments[selectedIndex]);
                      
                    }}
                  >
                    {apiDeployments.map((deployment: ApiDeployment, in_idx: number) => (
                      <MenuItem
                        key={`${deployment.site.host}_${deployment.site.subdomain}`}
                        value={in_idx}
                      >
                        {`${deployment.site.subdomain}.${deployment.site.host}`}
                      </MenuItem>
                    ))}
                  </Select>
                  {curl && <JsonEditor json={curl}/>}
        </>
        
      ) : (
        <Typography>No Route Found!</Typography>
      )}
    </div>
  );
}
