/* eslint-disable @typescript-eslint/no-explicit-any */
"use client";

import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { z } from "zod";
import { useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import {
  Form,
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
} from "@/components/ui/form";
import { Textarea } from "@/components/ui/textarea";
import { useEffect, useState } from "react";
import { Component } from "@/types/component";
import { API } from "@/service";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { useNavigate } from "react-router-dom";
import { ArrowLeft } from "lucide-react";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { cn } from "@/lib/utils";

const formSchema = z.object({
  name: z.string().min(2, {
    message: "Plugin name must be at least 2 characters.",
  }),
  version: z.string().regex(/^v\d+$/, {
    message: "Version must be in the format v{0-9}",
  }),
  description: z.string().min(10, {
    message: "Description must be at least 10 characters.",
  }),
  homepage: z.string().url({
    message: "Please enter a valid URL.",
  }),
  icon: z.instanceof(File),
  specs: z.discriminatedUnion("type", [
    z.object({
      type: z.literal("OplogProcessor"),
      componentId: z
        .string()
        .uuid({ message: "Component ID must be a valid UUID." }),
      componentVersion: z.number().min(0, {
        message: "Component version is mandatory",
      }),
    }),
    z.object({
      type: z.literal("ComponentTransformer"),
      validateUrl: z.string().url({ message: "Please enter a valid URL." }),
      transformUrl: z.string().url({ message: "Please enter a valid URL." }),
      jsonSchema: z.string().optional(),
    }),
  ]),
  scope: z.discriminatedUnion("type", [
    z.object({
      type: z.literal("Global"),
    }),
    z.object({
      type: z.literal("Component"),
      componentID: z
        .string()
        .uuid({ message: "Component ID must be a valid UUID." }),
    }),
  ]),
});

export default function CreatePlugin() {
  const navigate = useNavigate();
  const [componentApiList, setComponentApiList] = useState<{
    [key: string]: Component;
  }>({});
  const [activeSpecTab, setActiveSpecTab] = useState("OplogProcessor");
  const form = useForm<z.infer<typeof formSchema>>({
    resolver: zodResolver(formSchema),
    defaultValues: {
      name: "",
      version: "",
      description: "",
      homepage: "",
      specs: {
        type: "OplogProcessor",
        componentId: "",
      },
      scope: {
        type: "Global",
      },
    },
  });

  useEffect(() => {
    API.getComponentByIdAsKey().then(async (response) => {
      setComponentApiList(response);
    });
  }, []);

  useEffect(() => {
    form.setValue(
      "specs.type",
      activeSpecTab as "OplogProcessor" | "ComponentTransformer"
    );
  }, [activeSpecTab, form]);

  async function onSubmit(values: any) {
    values.icon = [];
    API.createPlugin(values).then(() => {
      navigate(`/plugins`);
    });
  }

  return (
    <div className="container mx-auto py-10">
      <Card className="max-w-4xl mx-auto">
        <CardHeader>
          <CardTitle className="text-3xl font-bold">
            Create a new Plugin
          </CardTitle>
          <CardDescription>
            Fill in the details to create your new plugin
          </CardDescription>
        </CardHeader>
        <CardContent>
          <Form {...form}>
            <form
              onSubmit={form.handleSubmit(onSubmit)}
              className="space-y-8 max-h-[calc(100vh-300px)] overflow-y-auto px-1"
            >
              <div className="grid gap-6 sm:grid-cols-2">
                <FormField
                  control={form.control}
                  name="name"
                  render={({ field }) => (
                    <FormItem>
                      <FormLabel>
                        Name<span className="text-red-500">*</span>
                      </FormLabel>
                      <FormControl>
                        <Input
                          placeholder="Plugin name"
                          {...field}
                          className={cn(
                            form.formState.errors.name &&
                              "border-red-500 focus-visible:ring-red-500"
                          )}
                        />
                      </FormControl>
                      <FormDescription>
                        Enter the name of your plugin.
                      </FormDescription>
                      <FormMessage />
                    </FormItem>
                  )}
                />
                <FormField
                  control={form.control}
                  name="version"
                  render={({ field }) => (
                    <FormItem>
                      <FormLabel>
                        Version<span className="text-red-500">*</span>
                      </FormLabel>
                      <FormControl>
                        <Input
                          placeholder="v0"
                          {...field}
                          className={cn(
                            form.formState.errors.version &&
                              "border-red-500 focus-visible:ring-red-500"
                          )}
                        />
                      </FormControl>
                      <FormDescription>
                        Enter the version in the format v0.
                      </FormDescription>
                      <FormMessage />
                    </FormItem>
                  )}
                />
              </div>
              <FormField
                control={form.control}
                name="description"
                render={({ field }) => (
                  <FormItem>
                    <FormLabel>
                      Description<span className="text-red-500">*</span>
                    </FormLabel>
                    <FormControl>
                      <Textarea
                        placeholder="Describe your plugin"
                        {...field}
                        className={cn(
                          form.formState.errors.description &&
                            "border-red-500 focus-visible:ring-red-500"
                        )}
                      />
                    </FormControl>
                    <FormDescription>
                      Provide a brief description of your plugin.
                    </FormDescription>
                    <FormMessage />
                  </FormItem>
                )}
              />
              <div className="grid gap-6 sm:grid-cols-2">
                <FormField
                  control={form.control}
                  name="icon"
                  render={() => (
                    <FormItem>
                      <FormLabel>
                        Icon<span className="text-red-500">*</span>
                      </FormLabel>
                      <FormControl>
                        <Input
                          type="file"
                          accept="image/*"
                          onChange={(e) => {
                            const file = e.target.files?.[0];
                            if (file) {
                              form.setValue("icon", file); // Update the form value manually
                            }
                          }}
                          className={cn(
                            form.formState.errors.icon &&
                              "border-red-500 focus-visible:ring-red-500"
                          )}
                        />
                      </FormControl>
                      <FormDescription>
                        Upload an icon for your plugin.
                      </FormDescription>
                      <FormMessage />
                    </FormItem>
                  )}
                />
                <FormField
                  control={form.control}
                  name="homepage"
                  render={({ field }) => (
                    <FormItem>
                      <FormLabel>
                        Homepage<span className="text-red-500">*</span>
                      </FormLabel>
                      <FormControl>
                        <Input
                          placeholder="https://example.com"
                          {...field}
                          className={cn(
                            form.formState.errors.homepage &&
                              "border-red-500 focus-visible:ring-red-500"
                          )}
                        />
                      </FormControl>
                      <FormDescription>
                        Enter the homepage URL for your plugin.
                      </FormDescription>
                      <FormMessage />
                    </FormItem>
                  )}
                />
              </div>

              <div className="grid gap-6 sm:grid-cols-2">
                <div className="space-y-6">
                  <h3 className="text-lg font-semibold">Specs</h3>
                  <Tabs
                    value={activeSpecTab}
                    onValueChange={setActiveSpecTab}
                    className="w-full"
                  >
                    <TabsList className="grid w-full grid-cols-2 mb-6">
                      <TabsTrigger value="OplogProcessor">
                        OplogProcessor
                      </TabsTrigger>
                      <TabsTrigger value="ComponentTransformer">
                        ComponentTransformer
                      </TabsTrigger>
                    </TabsList>
                    <TabsContent value="OplogProcessor">
                      <div className="space-y-4">
                        <FormField
                          control={form.control}
                          name="specs.componentId"
                          render={({ field }) => (
                            <FormItem>
                              <FormLabel>
                                Component ID
                                <span className="text-red-500">*</span>
                              </FormLabel>
                              <Select
                                value={field.value}
                                name={field.name}
                                onValueChange={field.onChange}
                              >
                                <FormControl>
                                  <SelectTrigger
                                    className={cn(
                                      (form.formState.errors.specs as any)
                                        ?.componentId &&
                                        "border-red-500 focus-visible:ring-red-500"
                                    )}
                                  >
                                    <SelectValue placeholder="Select a Component" />
                                  </SelectTrigger>
                                </FormControl>
                                <SelectContent>
                                  {componentApiList &&
                                    Object.values(componentApiList).map(
                                      (data) => (
                                        <SelectItem
                                          value={data.componentId!}
                                          key={data.componentName}
                                        >
                                          {data.componentName}
                                        </SelectItem>
                                      )
                                    )}
                                </SelectContent>
                              </Select>
                              <FormMessage />
                            </FormItem>
                          )}
                        />
                        <FormField
                          control={form.control}
                          name="specs.componentVersion"
                          render={({ field }) => (
                            <FormItem>
                              <FormLabel>
                                Component Version
                                <span className="text-red-500">*</span>
                              </FormLabel>
                              <Select
                                value={field.value?.toString()}
                                name={field.name}
                                onValueChange={(e) => field.onChange(Number(e))}
                              >
                                <FormControl>
                                  <SelectTrigger
                                    className={cn(
                                      (form.formState.errors.specs as any)
                                        ?.componentVersion &&
                                        "border-red-500 focus-visible:ring-red-500"
                                    )}
                                  >
                                    <SelectValue placeholder="Select a version">
                                      V{field.value}
                                    </SelectValue>
                                  </SelectTrigger>
                                </FormControl>
                                <SelectContent>
                                  {componentApiList[
                                    form.watch("specs.componentId")
                                  ]?.versionId?.map((v: string) => (
                                    <SelectItem key={v} value={v}>
                                      V{v}
                                    </SelectItem>
                                  ))}
                                </SelectContent>
                              </Select>
                              <FormMessage />
                            </FormItem>
                          )}
                        />
                      </div>
                    </TabsContent>
                    <TabsContent value="ComponentTransformer">
                      <div className="space-y-4">
                        <FormField
                          control={form.control}
                          name="specs.validateUrl"
                          render={({ field }) => (
                            <FormItem>
                              <FormLabel>
                                Validate URL
                                <span className="text-red-500">*</span>
                              </FormLabel>
                              <FormControl>
                                <Input
                                  placeholder="https://api.example.com/validate"
                                  {...field}
                                  className={cn(
                                    (form.formState.errors.specs as any)
                                      ?.validateUrl &&
                                      "border-red-500 focus-visible:ring-red-500"
                                  )}
                                />
                              </FormControl>
                              <FormDescription>
                                Enter the URL for validating your plugin.
                              </FormDescription>
                              <FormMessage />
                            </FormItem>
                          )}
                        />
                        <FormField
                          control={form.control}
                          name="specs.transformUrl"
                          render={({ field }) => (
                            <FormItem>
                              <FormLabel>
                                Transform URL
                                <span className="text-red-500">*</span>
                              </FormLabel>
                              <FormControl>
                                <Input
                                  placeholder="https://api.example.com/transform"
                                  {...field}
                                  className={cn(
                                    (form.formState.errors.specs as any)
                                      ?.transformUrl &&
                                      "border-red-500 focus-visible:ring-red-500"
                                  )}
                                />
                              </FormControl>
                              <FormDescription>
                                Enter the URL for transforming your plugin.
                              </FormDescription>
                              <FormMessage />
                            </FormItem>
                          )}
                        />
                        <FormField
                          control={form.control}
                          name="specs.jsonSchema"
                          render={({ field }) => (
                            <FormItem>
                              <FormLabel>JSON Schema</FormLabel>
                              <FormControl>
                                <Textarea
                                  {...field}
                                  placeholder="Enter your JSON schema here..."
                                  className={cn(
                                    (form.formState.errors.specs as any)
                                      ?.jsonSchema &&
                                      "border-red-500 focus-visible:ring-red-500"
                                  )}
                                />
                              </FormControl>
                              <FormDescription>
                                Enter a valid JSON schema (optional).
                              </FormDescription>
                              <FormMessage />
                            </FormItem>
                          )}
                        />
                      </div>
                    </TabsContent>
                  </Tabs>
                </div>

                <div className="space-y-6">
                  <h3 className="text-lg font-semibold">Scope</h3>
                  <FormField
                    control={form.control}
                    name="scope.type"
                    render={({ field }) => (
                      <FormItem>
                        <FormLabel>
                          Scope Type<span className="text-red-500">*</span>
                        </FormLabel>
                        <FormControl>
                          <Select
                            value={field.value}
                            name={field.name}
                            onValueChange={field.onChange}
                          >
                            <FormControl>
                              <SelectTrigger
                                className={cn(
                                  form.formState.errors.scope?.type &&
                                    "border-red-500 focus-visible:ring-red-500"
                                )}
                              >
                                <SelectValue placeholder="Select a scope type" />
                              </SelectTrigger>
                            </FormControl>
                            <SelectContent>
                              <SelectItem value="Global">Global</SelectItem>
                              <SelectItem value="Component">
                                Component
                              </SelectItem>
                            </SelectContent>
                          </Select>
                        </FormControl>
                        <FormMessage />
                      </FormItem>
                    )}
                  />
                  {form.watch("scope.type") === "Component" && (
                    <FormField
                      control={form.control}
                      name="scope.componentID"
                      render={({ field }) => (
                        <FormItem className="mt-4">
                          <FormLabel>
                            Component ID<span className="text-red-500">*</span>
                          </FormLabel>
                          <Select
                            value={field.value}
                            name={field.name}
                            onValueChange={field.onChange}
                          >
                            <FormControl>
                              <SelectTrigger
                                className={cn(
                                  (form.formState.errors.scope as any)
                                    ?.componentID &&
                                    "border-red-500 focus-visible:ring-red-500"
                                )}
                              >
                                <SelectValue placeholder="Select a Component" />
                              </SelectTrigger>
                            </FormControl>
                            <SelectContent>
                              {componentApiList &&
                                Object.values(componentApiList).map((data) => (
                                  <SelectItem
                                    value={data.componentId!}
                                    key={data.componentName}
                                  >
                                    {data.componentName}
                                  </SelectItem>
                                ))}
                            </SelectContent>
                          </Select>
                          <FormMessage />
                        </FormItem>
                      )}
                    />
                  )}
                </div>
              </div>

              <div className="flex justify-between pt-6">
                <Button
                  type="button"
                  variant="outline"
                  onClick={() => navigate(-1)}
                >
                  <ArrowLeft className="mr-2 h-5 w-5" />
                  Back
                </Button>
                <Button type="submit">Create Plugin</Button>
              </div>
            </form>
          </Form>
        </CardContent>
      </Card>
    </div>
  );
}
