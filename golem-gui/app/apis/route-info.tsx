"use client";

import React, { useState } from "react";
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
import NewRouteForm from "../../components/new-route";
import useApiDefinitions from "@/lib/hooks/use-api-definitons";
import { useCustomParam } from "@/lib/hooks/use-custom-param";
import { AlertDialogDemo } from "../../components/confirmation-dialog";
import { useRouter } from "next/navigation";
import Link from "next/link";
import { TabsContent, TabsTrigger, TabsList, Tabs } from "../../components/ui/tabs";
import { Card, CardContent, CardHeader, CardTitle } from "../../components/ui/card";
import useComponents from "@/lib/hooks/use-component";


const ApiDetails = ({
  route,
  version,
  noRedirect,
  isDraft,
}: {
  route: ApiRoute;
  version: string;
  noRedirect?: boolean;
  isDraft?: boolean;
}) => {
  const { apiId } = useCustomParam();
  const { components, error, isLoading } = useComponents(
    route?.binding?.componentId?.componentId,
    "latest"
  );

  const router = useRouter();
  const { deleteRoute } = useApiDefinitions(apiId);
  const handleDelete = async (
    e: React.MouseEvent<HTMLButtonElement>
  ): Promise<void> => {
    console.log("delete route");
    e.preventDefault();
    try {
      await deleteRoute(route!, version);
      if (!noRedirect) {
        router.push(`/apis/${apiId}/overview?version=${version}`);
      }
    } catch (error) {
      throw error;
    }
  };
  const [activeTab, setActiveTab] = useState(0);
  const handleTabChange = (event: React.SyntheticEvent, newValue: number) => {
    setActiveTab(newValue);
  };

  // Recursive function to parse and render the structure
  const parseStructure = (fields: any[], parentKey?: string): JSX.Element[] => {
    return fields.map((field) => {
      const fieldType = field.typ;

      if (fieldType.fields) {
        // If the field contains nested fields
        return (
          <Box key={field.name} sx={{ marginLeft: 2 }}>
            <Typography variant="body1">{field.name}: &#123;</Typography>
            {parseStructure(fieldType.fields)}
            <Typography>&#125;</Typography>
          </Box>
        );
      } else if (fieldType.type === "Option") {
        // If the field is optional
        return (
          <Typography key={field.name} variant="body1" sx={{ marginLeft: 2 }}>
            {field.name}: {fieldType.inner.type} | null
          </Typography>
        );
      } else {
        // Base case for non-nested fields
        return (
          <Typography key={field.name} variant="body1" sx={{ marginLeft: 2 }}>
            {field.name}: {fieldType.type}
          </Typography>
        );
      }
    });
  };

  const bodyStructure =
    route?.binding?.responseMappingInput?.types.request?.fields;

  const paramStructure =
    route?.binding?.workerNameInput?.types?.request?.fields;
  console.log("route ", route);

  return (
    <>
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
              onSubmit={(e: React.MouseEvent<HTMLButtonElement>) =>
                handleDelete(e)
              }
              paragraph={
                "This action cannot be undone. This will permanently delete this route."
              }
              child={
                <Button variant="error" size="sm" endIcon={<Trash />}>
                  {" "}
                  Delete{" "}
                </Button>
              }
            />
          </Box>
        </Box>
        <Grid container spacing={2}>
          {/* Component */}
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
                  {components?.[0]?.componentName ??
                    route?.binding?.componentId?.componentId}
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
            <TabsTrigger value="view"> View </TabsTrigger>
            <TabsTrigger value="edit"> Edit </TabsTrigger>
            <TabsTrigger value="try-it-out"> Try-it-out </TabsTrigger>
          </TabsList>

          <TabsContent value="view">
            <Card>
              <CardHeader>
                <CardTitle>Route Details</CardTitle>
              </CardHeader>
              <CardContent>
                <>
                  {paramStructure && (
                    <>
                      <Grid size={{ xs: 12, sm: 3 }}>
                        <Typography
                          variant="body2"
                          className="text-muted-foreground"
                        >
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
                  {/*TODO: Request Body */}
                  {bodyStructure && (
                    <>
                      <Grid size={{ xs: 12, sm: 3 }}>
                        <Typography
                          variant="body2"
                          className="text-muted-foreground"
                        >
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
                        <Button
                          variant="primary"
                          size="icon_sm"
                          className="font-mono w-fit"
                        >
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

                  {/* Worker Name */}
                  <Grid size={{ xs: 12, sm: 3 }}>
                    <Typography variant="body2">
                      <Box display="flex" flexDirection="column" gap={1}>
                        <span className="text-muted-foreground">
                          Worker Name
                        </span>
                        <Button
                          variant="primary"
                          size="icon_sm"
                          className="font-mono w-fit"
                        >
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
                      <br />
                    </Paper>
                  </Grid>
                </>
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
                    onSuccess={() => setActiveTab(0)}
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
    </>
  );
};

export default ApiDetails;
