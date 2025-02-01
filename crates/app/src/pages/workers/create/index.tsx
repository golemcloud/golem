/* eslint-disable @typescript-eslint/ban-ts-comment */
/* eslint-disable @typescript-eslint/no-explicit-any */
// @ts-nocheck
import { useState, useEffect } from "react";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardTitle,
} from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import ComponentLeftNav from "../../components/details/componentsLeftNav";
import { z } from "zod";
import { useFieldArray, useForm } from "react-hook-form";
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
import { API } from "@/service";
import { v4 as uuidv4 } from "uuid";
import ErrorBoundary from "@/components/errorBoundary";
import { ArrowLeft } from "lucide-react";
import { useNavigate, useParams } from "react-router-dom";
import { Component } from "@/types/component";

const formSchema = z.object({
  componentID: z.string(),
  name: z.string().min(4, {
    message: "worker name must be at least 4 characters",
  }),
  env: z.array(
    z.object({
      key: z.string(),
      value: z.string(),
    })
  ),
  args: z.array(z.string()),
});

export default function CreateWorker() {
  const navigate = useNavigate();
  const { componentId } = useParams();
  const form = useForm<z.infer<typeof formSchema>>({
    resolver: zodResolver(formSchema),
    defaultValues: {
      componentID: componentId,
      name: "",
      env: [{ key: "", value: "" }],
      args: [" "],
    },
  });

  const [component, setComponent] = useState({} as Component);

  useEffect(() => {
    if (componentId) {
      API.getComponentByIdAsKey().then((response) => {
        setComponent(response[componentId]);
      });
    }
  }, [componentId]);

  const {
    fields: envFields,
    append: appendEnv,
    remove: removeEnv,
  } = useFieldArray({
    control: form.control,
    name: "env",
  });

  const {
    fields: argFields,
    append: appendArg,
    remove: removeArg,
  } = useFieldArray({
    control: form.control,
    name: "args",
  });

  function generateUUID() {
    form.setValue("name", uuidv4());
  }

  function onSubmit(values: z.infer<typeof formSchema>) {
    const { componentID, ...rest } = values as any;
    rest.env = rest.env.reduce(
      (acc: Record<string, string>, arg: { key: string; value: string }) => {
        if (arg.key) {
          acc[arg.key] = arg.value;
        }
        return acc;
      },
      {}
    );
    rest.args = rest.args.filter((x: any) => x && x.trim().length > 0);

    API.createWorker(componentID, rest).then((response) => {
      navigate(
        `/components/${componentId}/workers/${response.workerId.workerName}`
      );
    });
  }

  return (
    <ErrorBoundary>
      <div className="flex">
        <ComponentLeftNav componentDetails={component} />
        <div className="flex-1 flex flex-col">
          <header className="w-full border-b bg-background py-4">
            <div className="mx-auto px-6 lg:px-8">
              <div className="flex items-center gap-4">
                <h1 className="text-xl font-semibold text-foreground truncate">
                  {component.componentName}
                </h1>
              </div>
            </div>
          </header>
          <div className="flex-1 p-8">
            <div className="p-4 min-h-screen bg-background text-foreground mx-auto max-w-7xl p-10">
              <Card className="max-w-2xl mx-auto border-0 shadow-none">
                <CardTitle>
                  <h1 className="text-2xl font-semibold mb-1">
                    Create a new Worker
                  </h1>
                </CardTitle>
                <CardDescription>
                  <p className="text-sm text-gray-400">Launch a new worker</p>
                </CardDescription>
                <CardContent className="py-6 px-0">
                  <Form {...form}>
                    <form
                      onSubmit={form.handleSubmit(onSubmit)}
                      className="space-y-8"
                    >
                      <FormField
                        control={form.control}
                        name="name"
                        render={({ field }) => (
                          <FormItem>
                            <FormLabel>Worker Name</FormLabel>
                            <FormControl>
                              <div className="flex gap-2">
                                <Input {...field} />
                                <Button
                                  type="button"
                                  variant="secondary"
                                  onClick={generateUUID}
                                >
                                  Generate
                                </Button>
                              </div>
                            </FormControl>
                            <FormDescription>
                              The name must be unique for this component.
                            </FormDescription>
                            <FormMessage />
                          </FormItem>
                        )}
                      />
                      <div>
                        <FormLabel>Environment Variables</FormLabel>
                        {envFields.map((field, index) => (
                          <div
                            key={field.id}
                            className="flex items-center gap-2 pt-2"
                          >
                            <FormField
                              control={form.control}
                              name={`env.${index}.key`}
                              render={({ field }) => (
                                <FormControl>
                                  <Input placeholder="Key" {...field} />
                                </FormControl>
                              )}
                            />
                            <FormField
                              control={form.control}
                              name={`env.${index}.value`}
                              render={({ field }) => (
                                <FormControl>
                                  <Input
                                    placeholder="Value"
                                    {...field}
                                    type="password"
                                  />
                                </FormControl>
                              )}
                            />
                            <Button
                              type="button"
                              variant="secondary"
                              size="sm"
                              disabled={envFields.length <= 1}
                              onClick={() =>
                                envFields.length > 1 && removeEnv(index)
                              }
                            >
                              Remove
                            </Button>
                          </div>
                        ))}
                        <Button
                          className={"mt-4"}
                          type="button"
                          variant="outline"
                          size="sm"
                          onClick={() => appendEnv({ key: "", value: "" })}
                        >
                          Add Environment Variable
                        </Button>
                      </div>
                      <div>
                        <FormLabel>Arguments</FormLabel>

                        {argFields.map((field, index) => (
                          <div
                            key={field.id}
                            className="flex items-center gap-2 pb-2"
                          >
                            <FormField
                              control={form.control}
                              name={`args.${index}`}
                              render={({ field }) => (
                                <FormControl>
                                  <Input {...field} />
                                </FormControl>
                              )}
                            />
                            <Button
                              type="button"
                              variant="secondary"
                              size="sm"
                              disabled={argFields.length <= 1}
                              onClick={() =>
                                argFields.length > 1 && removeArg(index)
                              }
                            >
                              Remove
                            </Button>
                          </div>
                        ))}
                        <Button
                          className={"mt-2"}
                          type="button"
                          variant="outline"
                          size="sm"
                          onClick={() => appendArg("")}
                        >
                          Add Arguments
                        </Button>
                      </div>

                      <div className="flex justify-between">
                        <Button
                          type="button"
                          variant="secondary"
                          onClick={() => navigate(-1)}
                        >
                          <ArrowLeft className="mr-2 h-5 w-5" />
                          Back
                        </Button>
                        <Button type="submit">Submit</Button>
                      </div>
                    </form>
                  </Form>
                </CardContent>
              </Card>
            </div>
          </div>
        </div>
      </div>
    </ErrorBoundary>
  );
}
