import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import {
  Form,
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
} from "@/components/ui/form";

import { useEffect, useState } from "react";

import { API } from "@/service";
import { ArrowLeft, FileIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { ComponentList } from "@/types/component";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { cn } from "@/lib/utils";
import { toast } from "@/hooks/use-toast";
import { useForm } from "react-hook-form";
import { useNavigate, useParams } from "react-router-dom";
import { z } from "zod";
import { zodResolver } from "@hookform/resolvers/zod";
import { open } from "@tauri-apps/plugin-dialog";

const formSchema = z.object({
  name: z
    .string()
    .min(2, {
      message: "Plugin name must be at least 2 characters.",
    })
    .regex(/^[a-z][a-z0-9-]*$/, {
      message:
        "Plugin name must be lowercase, start with a letter, and contain only letters, numbers, and hyphens.",
    }),
  version: z.string().regex(/^\d+\.\d+\.\d+$/, {
    message: "Version must be in the format 0.0.1",
  }),
  description: z.string().min(10, {
    message: "Description must be at least 10 characters.",
  }),
  homepage: z.string().url({
    message: "Please enter a valid URL.",
  }),
  icon: z.string().min(1, {
    message: "Icon path is required.",
  }),
  specs: z
    .object({
      type: z.enum(["OplogProcessor"]),
      // OplogProcessor specific field
      component: z.string().optional(),
    })
    .refine(
      data => {
        if (data.type === "OplogProcessor") {
          return data.component;
        }
        return true;
      },
      {
        message: "Required fields for selected type are missing",
      },
    ),
});

export type CreatePluginFormData = z.infer<typeof formSchema>;

export default function CreatePlugin() {
  const navigate = useNavigate();
  const { appId } = useParams<{ appId: string }>();
  const [_componentApiList, setComponentApiList] = useState<{
    [key: string]: ComponentList;
  }>({});
  const form = useForm<z.infer<typeof formSchema>>({
    resolver: zodResolver(formSchema),
    defaultValues: {
      name: "my-plugin",
      version: "0.0.1",
      description: "",
      homepage: "",
      icon: "",
      specs: {
        type: "OplogProcessor",
        component: "",
      },
    },
  });

  useEffect(() => {
    API.componentService.getComponentByIdAsKey(appId!).then(async response => {
      setComponentApiList(response);
    });
  }, []);

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
              onSubmit={form.handleSubmit(
                async (data: CreatePluginFormData) => {
                  try {
                    await API.pluginService.createPlugin(appId!, data);

                    navigate(`/app/${appId}/plugins`);
                    toast({
                      title: "Plugin created successfully",
                      description:
                        "Plugin has been registered and is now available.",
                      duration: 3000,
                    });
                  } catch (error: unknown) {
                    toast({
                      title: "Failed to create plugin",
                      description:
                        error instanceof Error ? error.message : String(error),
                      variant: "destructive",
                      duration: 5000,
                    });
                  }
                },
              )}
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
                          placeholder="my-plugin-name"
                          {...field}
                          className={cn(
                            form.formState.errors.name &&
                              "border-red-500 focus-visible:ring-red-500",
                          )}
                        />
                      </FormControl>
                      <FormDescription>
                        Plugin name must be lowercase with no spaces (use
                        hyphens instead).
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
                          placeholder="0.0.1"
                          {...field}
                          className={cn(
                            form.formState.errors.version &&
                              "border-red-500 focus-visible:ring-red-500",
                          )}
                        />
                      </FormControl>
                      <FormDescription>
                        Enter the version in the format 0.0.1.
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
                            "border-red-500 focus-visible:ring-red-500",
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
                              "border-red-500 focus-visible:ring-red-500",
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
                <FormField
                  control={form.control}
                  name="icon"
                  render={({ field }) => (
                    <FormItem>
                      <FormLabel>
                        Icon Path<span className="text-red-500">*</span>
                      </FormLabel>
                      <FormControl>
                        <div className="flex gap-2">
                          <Input
                            placeholder="path/to/icon.png"
                            {...field}
                            readOnly
                            className={cn(
                              form.formState.errors.icon &&
                                "border-red-500 focus-visible:ring-red-500",
                            )}
                          />
                          <Button
                            type="button"
                            variant="outline"
                            size="icon"
                            onClick={async () => {
                              try {
                                const selected = await open({
                                  multiple: false,
                                  filters: [
                                    {
                                      name: "Image",
                                      extensions: [
                                        "png",
                                        "jpg",
                                        "jpeg",
                                        "gif",
                                        "svg",
                                        "ico",
                                        "webp",
                                      ],
                                    },
                                  ],
                                });
                                if (selected) {
                                  field.onChange(selected);
                                }
                              } catch (error) {
                                console.error("Error selecting file:", error);
                              }
                            }}
                          >
                            <FileIcon className="h-4 w-4" />
                          </Button>
                        </div>
                      </FormControl>
                      <FormDescription>
                        Select an icon file for your plugin.
                      </FormDescription>
                      <FormMessage />
                    </FormItem>
                  )}
                />
              </div>

              {/* Plugin Type */}
              <div className="space-y-4">
                <h3 className="text-lg font-semibold">Plugin Type</h3>
                <p className="text-sm text-muted-foreground">Oplog Processor</p>
              </div>

              <div className="space-y-4">
                <h3 className="text-lg font-semibold">
                  Oplog Processor Configuration
                </h3>
                <FormField
                  control={form.control}
                  name="specs.component"
                  render={({ field }) => (
                    <FormItem>
                      <FormLabel>
                        Component Path<span className="text-red-500">*</span>
                      </FormLabel>
                      <FormControl>
                        <div className="flex gap-2">
                          <Input
                            placeholder="path/to/oplog-processor.wasm"
                            {...field}
                            readOnly
                            className={cn(
                              form.formState.errors.specs?.component &&
                                "border-red-500 focus-visible:ring-red-500",
                            )}
                          />
                          <Button
                            type="button"
                            variant="outline"
                            size="icon"
                            onClick={async () => {
                              try {
                                const selected = await open({
                                  multiple: false,
                                  filters: [
                                    {
                                      name: "WASM Component",
                                      extensions: ["wasm"],
                                    },
                                  ],
                                });
                                if (selected) {
                                  field.onChange(selected);
                                }
                              } catch (error) {
                                console.error("Error selecting file:", error);
                              }
                            }}
                          >
                            <FileIcon className="h-4 w-4" />
                          </Button>
                        </div>
                      </FormControl>
                      <FormDescription>
                        Select the WASM component file
                      </FormDescription>
                      <FormMessage />
                    </FormItem>
                  )}
                />
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
