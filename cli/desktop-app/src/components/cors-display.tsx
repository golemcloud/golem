import { Badge } from "@/components/ui/badge";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { HttpCors } from "@/types/api";
import { Clock, Globe, Shield } from "lucide-react";

export function CorsDisplay({ cors }: { cors: HttpCors }) {
  const corsData = cors;

  return (
    <div className="grid gap-6">
      <div className="flex flex-col gap-2">
        <h1 className="text-1xl font-bold tracking-tight">
          CORS Configuration
        </h1>
        <p className="text-1xl text-muted-foreground">
          Manage Cross-Origin Resource Sharing settings for your API
        </p>
      </div>

      <div className="grid gap-6 md:grid-cols-3">
        <Card>
          <CardHeader className="space-y-1">
            <CardTitle className="text-1xl">Origins</CardTitle>
            <CardDescription>Allowed origin configuration</CardDescription>
          </CardHeader>
          <CardContent>
            <div className="flex items-center gap-2">
              <Globe className="h-4 w-4 text-muted-foreground" />
              <code className="relative rounded bg-muted px-[0.3rem] py-[0.2rem] font-mono text-sm">
                {corsData.allowOrigin}
              </code>
            </div>
          </CardContent>
        </Card>

        <Card>
          <CardHeader className="space-y-1">
            <CardTitle className="text-1xl">Security</CardTitle>
            <CardDescription>Credentials and headers</CardDescription>
          </CardHeader>
          <CardContent>
            <div className="flex items-center gap-2">
              <Shield className="h-4 w-4 text-muted-foreground" />
              <Badge
                variant={corsData.allowCredentials ? "default" : "secondary"}
              >
                {corsData.allowCredentials
                  ? "Credentials Allowed"
                  : "No Credentials"}
              </Badge>
            </div>
          </CardContent>
        </Card>

        <Card>
          <CardHeader className="space-y-1">
            <CardTitle className="text-1xl">Cache</CardTitle>
            <CardDescription>Preflight cache duration</CardDescription>
          </CardHeader>
          <CardContent>
            <div className="flex items-center gap-2">
              <Clock className="h-4 w-4 text-muted-foreground" />
              <Badge variant="outline" className="font-mono">
                {corsData.maxAge}s
              </Badge>
            </div>
          </CardContent>
        </Card>
      </div>

      <Card>
        <CardHeader>
          <CardTitle>Detailed Configuration</CardTitle>
          <CardDescription>
            Complete CORS preflight configuration settings
          </CardDescription>
        </CardHeader>
        <CardContent>
          <Tabs defaultValue="table" className="w-full">
            <TabsList className="grid w-full max-w-[400px] grid-cols-2">
              <TabsTrigger value="table">Table View</TabsTrigger>
              <TabsTrigger value="code">Code View</TabsTrigger>
            </TabsList>
            <TabsContent value="table">
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead className="w-[200px]">Property</TableHead>
                    <TableHead>Value</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {Object.entries(corsData).map(([key, value]) => (
                    <TableRow key={key}>
                      <TableCell className="font-mono text-sm">{key}</TableCell>
                      <TableCell>
                        {typeof value === "boolean" ? (
                          <Badge variant={value ? "default" : "secondary"}>
                            {value.toString()}
                          </Badge>
                        ) : typeof value === "number" ? (
                          <Badge variant="outline" className="font-mono">
                            {value}
                          </Badge>
                        ) : (
                          <code className="relative rounded bg-muted px-[0.3rem] py-[0.2rem] font-mono text-sm">
                            {value}
                          </code>
                        )}
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </TabsContent>
            <TabsContent value="code">
              <pre className="rounded-lg bg-muted p-4 overflow-x-auto">
                <code className="text-sm font-mono">
                  {JSON.stringify(corsData, null, 2)}
                </code>
              </pre>
            </TabsContent>
          </Tabs>
        </CardContent>
      </Card>
    </div>
  );
}
