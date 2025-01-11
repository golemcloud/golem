import DynamicForm from "@/app/components/[compId]/workers/[id]/form-generator";
import { ApiRoute, Parameter } from "@/types/api";
import {
  AnalysedType_TypeOption,
  AnalysedType_TypeRecord,
} from "@/types/golem-data-types";
import { Typography } from "@mui/material";
import React, { useMemo } from "react";
type FormData = {
  [key: string]: unknown;
};
export default function TryItOut({ route }: { route: ApiRoute }) {
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
            type: "Option",
          } as AnalysedType_TypeOption,
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
            type: "Option",
          } as AnalysedType_TypeOption,
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
            type: "Option",
          } as AnalysedType_TypeOption,
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
            type: "Option",
          } as AnalysedType_TypeOption,
        });
      }
    }

    return meta;
  }, [route]);

  console.log("routeMeta====>", routeMeta);

  const handleSubmit = (data: FormData) => {
    console.log("data==========>", data);
  };
  return (
    <div>
      {route && routeMeta ? (
        <DynamicForm config={[routeMeta]} onSubmit={handleSubmit} />
      ) : (
        <Typography>No Route Found!</Typography>
      )}
    </div>
  );
}
