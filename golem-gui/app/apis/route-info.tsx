"use client";

import React from "react";
import {
  Box,
  Typography,
  Grid2 as Grid,
  Paper,
  Divider,
  Stack,
} from "@mui/material";
import { Button2 as Button } from "@/components/ui/button";
import { Trash } from "lucide-react";
import { ApiRoute } from "@/types/api";
import TryItOut from "./try-it-out";
import NewRouteForm from "./new-route";
import { useApiDefinitions } from "@/components/imports"; 
import { useCustomParam } from "@/lib/hooks/use-custom-param";
import { AlertDialogDemo } from "../../components/ui/confirmation-dialog";
import { useRouter } from "next/navigation";
import Link from "next/link";
import { TabsContent, TabsTrigger, TabsList, Tabs } from "../../components/ui/tabs";
import { Card, CardContent, CardHeader, CardTitle } from "../../components/ui/card";
import useComponents from "@/lib/hooks/use-component";

interface ApiDetailsProps {
  route: ApiRoute;
  version: string;
  noRedirect?: boolean;
  isDraft?: boolean;
}

interface FieldType {
  type: string;
  fields?: Field[];
  inner?: { type: string };
}

interface Field {
  name: string;
  typ: FieldType;
}

const ApiDetails: React.FC<ApiDetailsProps> = ({
  route,
  version,
  noRedirect,
  isDraft,
}) => {
  const { apiId } = useCustomParam();
  const { components, isLoading } = useComponents(
    route?.binding?.componentId?.componentId,
    "latest"
  );

  console.log("draft", isDraft);

  const router = useRouter();
  const { deleteRoute } = useApiDefinitions(apiId);

  const handleDelete = async (e: React.MouseEvent<HTMLButtonElement>): Promise<void> => {
    e.preventDefault();
    try {
      await deleteRoute(route, version);
      if (!noRedirect) {
        router.push(`/apis/${apiId}/overview?version=${version}`);
      }
    } catch (error) {
      throw error;
    }
  };

  const parseStructure = (fields: Field[]): JSX.Element[] => {
    return fields.map((field) => {
      const fieldType = field.typ;

      if (fieldType.fields) {
        return (
          <Box key={field.name} sx={{ marginLeft: 2 }}>
            <Typography variant="body1">{field.name}: &#123;</Typography>
            {parseStructure(fieldType.fields)}
            <Typography>&#125;</Typography>
          </Box>
        );
      } else if (fieldType.type === "Option") {
        return (
          <Typography key={field.name} variant="body1" sx={{ marginLeft: 2 }}>
            {field.name}: {fieldType.inner?.type ?? "unknown"} | null
          </Typography>
        );
      } else {
        return (
          <Typography key={field.name} variant="body1" sx={{ marginLeft: 2 }}>
            {field.name}: {fieldType.type}
          </Typography>
        );
      }
    });
  };

  // @ts-expect-error - The structure of `responseMappingInput` is not fully typed yet
const bodyStructure = route?.binding?.responseMappingInput?.types.request?.fields;
// @ts-expect-error - The structure of `workerNameInput` is not fully typed yet
const paramStructure = route?.binding?.workerNameInput?.types?.request?.fields;

  return (
    <Box>
      <Box className="flex justify-between">
        <Box>
          <Typography variant="h5">{route?.path}</Typography>
          <Button variant="primary" size="icon_sm">
            {route?.method}
          </Button>
        </Box>
        <Box sx={{ display: "flex", gap: 1 }}>
          <AlertDialogDemo
            // @ts-expect-error - The structure of `result` is not fully typed yet
            onSubmit={handleDelete}
            paragraph="This action cannot be undone. This will permanently delete this route."
            child={
              <Button variant="error" size="sm" endIcon={<Trash />}>
                Delete
              </Button>
            }
          />
        </Box>
      </Box>
      <Grid container spacing={2}>
        <Grid size={12}>
          <Divider className="bg-border my-2" />
        </Grid>
        <Grid size={{ xs: 12, sm: 3 }} alignItems="center">
          <Typography variant="body2" className="text-muted-foreground">
            Component
          </Typography>
        </Grid>
        <Grid size={{ xs: 12, sm: 9 }} alignItems="center">
          <Link
            href={`/components/${route?.binding?.componentId?.componentId}/overview?version=${route?.binding?.componentId?.version}`}
          >
            {!isLoading && (
              <Typography variant="caption" fontFamily="monospace">
                {components?.[0]?.componentName ?? route?.binding?.componentId?.componentId}
                {"/"}
                {route?.binding?.componentId?.version}
              </Typography>
            )}
          </Link>
        </Grid>
        <Grid size={12}>
          <Divider className="bg-border my-2" />
        </Grid>
      </Grid>

      <Tabs defaultValue="view" className="w-full">
        <TabsList>
          <TabsTrigger value="view">View</TabsTrigger>
          <TabsTrigger value="edit">Edit</TabsTrigger>
          <TabsTrigger value="try-it-out">Try-it-out</TabsTrigger>
        </TabsList>

        <TabsContent value="view">
          <Card>
            <CardHeader>
              <CardTitle>Route Details</CardTitle>
            </CardHeader>
            <CardContent>
              {paramStructure && (
                <>
                  <Grid size={{ xs: 12, sm: 3 }}>
                    <Typography variant="body2" className="text-muted-foreground">
                      Path Parameters
                    </Typography>
                  </Grid>
                  <Grid size={{ xs: 12, sm: 9 }}>
                    <Stack direction="row" gap={5} alignItems="center">
                      <Paper
                        elevation={0}
                        className="w-full"
                        sx={{
                          p: 2,
                          fontFamily: "monospace",
                          fontSize: "0.875rem",
                        }}
                      >
                        {parseStructure(paramStructure)}
                      </Paper>
                    </Stack>
                  </Grid>
                  <Grid size={12}>
                    <Divider className="bg-border my-2" />
                  </Grid>
                </>
              )}
              {bodyStructure && (
                <>
                  <Grid size={{ xs: 12, sm: 3 }}>
                    <Typography variant="body2" className="text-muted-foreground">
                      Request Body
                    </Typography>
                  </Grid>
                  <Grid size={{ xs: 12, sm: 9 }}>
                    <Paper
                      elevation={0}
                      sx={{
                        p: 2,
                        fontFamily: "monospace",
                        fontSize: "0.875rem",
                      }}
                    >
                      {parseStructure(bodyStructure)}
                    </Paper>
                  </Grid>
                  <Grid size={12}>
                    <Divider className="bg-border my-2" />
                  </Grid>
                </>
              )}
              <Grid size={{ xs: 12, sm: 3 }}>
                <Typography variant="body2">
                  <Box display="flex" flexDirection="column" gap={1}>
                    <span className="text-muted-foreground">Response</span>
                    <Button variant="primary" size="icon_sm" className="font-mono w-fit">
                      Rib
                    </Button>
                  </Box>
                </Typography>
              </Grid>
              <Grid size={{ xs: 12, sm: 9 }}>
                <Paper
                  elevation={0}
                  sx={{
                    p: 2,
                    fontFamily: "monospace",
                    fontSize: "0.875rem",
                  }}
                >
                  {route?.binding?.response}
                </Paper>
              </Grid>
              <Grid size={12}>
                <Divider className="bg-border my-2" />
              </Grid>
              <Grid size={{ xs: 12, sm: 3 }}>
                <Typography variant="body2">
                  <Box display="flex" flexDirection="column" gap={1}>
                    <span className="text-muted-foreground">Worker Name</span>
                    <Button variant="primary" size="icon_sm" className="font-mono w-fit">
                      Rib
                    </Button>
                  </Box>
                </Typography>
              </Grid>
              <Grid size={{ xs: 12, sm: 9 }}>
                <Paper
                  elevation={0}
                  sx={{
                    p: 2,
                    fontFamily: "monospace",
                    fontSize: "0.875rem",
                  }}
                >
                  {route?.binding?.workerName}
                </Paper>
              </Grid>
            </CardContent>
          </Card>
        </TabsContent>

        <TabsContent value="edit">
          <Card>
            <CardHeader>
              <CardTitle>Edit Route</CardTitle>
            </CardHeader>
            <CardContent>
              {route && (
                <NewRouteForm
                  apiId={apiId}
                  version={version}
                  defaultRoute={route}
                  noRedirect={noRedirect}
                />
              )}
            </CardContent>
          </Card>
        </TabsContent>

        <TabsContent value="try-it-out">
          <Card>
            <CardHeader>
              <CardTitle>Try-it-out</CardTitle>
            </CardHeader>
            <CardContent>
              {route && <TryItOut route={route} version={version} />}
            </CardContent>
          </Card>
        </TabsContent>
      </Tabs>
    </Box>
  );
};

export default ApiDetails;