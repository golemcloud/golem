import { useParams } from "react-router-dom";
import { useEffect, useState, useRef } from "react";

import ComponentLeftNav from "./componentsLeftNav";
import {
  Card,
  CardContent,
  CardDescription,
  CardTitle,
} from "@/components/ui/card.tsx";
import {
  Form,
  FormControl,
  FormField,
  FormItem,
  FormLabel,
} from "@/components/ui/form.tsx";
import { Input } from "@/components/ui/input.tsx";
import { FileUp } from "lucide-react";
import { Button } from "@/components/ui/button.tsx";
import { useForm } from "react-hook-form";
import { z } from "zod";
import { zodResolver } from "@hookform/resolvers/zod";
import { API } from "@/service";
import { toast } from "@/hooks/use-toast.ts";
import ErrorBoundary from "@/components/errorBoundary.tsx";
import { Component } from "@/types/component";

const formSchema = z.object({
  component: z.instanceof(File).refine((file) => file.size < 50000000, {
    message: "Your resume must be less than 50MB.",
  }),
});

export default function ComponentUpdate() {
  const { componentId } = useParams();
  const [file, setFile] = useState<File | null>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const form = useForm<z.infer<typeof formSchema>>({
    resolver: zodResolver(formSchema),
    defaultValues: {
      component: undefined,
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

  function onSubmit() {
    const formData = new FormData();
    formData.append("component", file!);
    API.updateComponent(componentId!, formData).then(() => {
      form.reset();
      setFile(null);
      toast({
        title: "Component was updated successfully",
        duration: 3000,
      });
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
            <Card
              className="max-w-4xl mx-auto border-0 shadow-none"
              key={"component.componentName"}
            >
              <CardTitle>
                <h1 className="text-2xl font-semibold mb-1">
                  Update Component
                </h1>
              </CardTitle>
              <CardDescription>
                <p className="text-sm text-gray-400">
                  Components are the building blocks
                </p>
              </CardDescription>
              <CardContent className="p-6">
                <Form {...form}>
                  <form
                    onSubmit={form.handleSubmit(onSubmit)}
                    className="space-y-8"
                  >
                    <FormField
                      control={form.control}
                      name="component"
                      render={({ field: { onChange, onBlur, name, ref } }) => (
                        <FormItem>
                          <FormLabel>Component</FormLabel>
                          <FormControl>
                            <div
                              className="border-2 border-dashed border-gray-200 rounded-lg p-8 cursor-pointer hover:border-gray-400"
                              onClick={() => fileInputRef?.current?.click()}
                            >
                              <div className="flex flex-col items-center justify-center text-center">
                                <FileUp className="h-8 w-8 text-gray-400 mb-3" />
                                <Input
                                  type="file"
                                  accept="application/wasm,.wasm"
                                  className="hidden"
                                  name={name}
                                  onBlur={onBlur}
                                  ref={(e) => {
                                    ref(e); // Forward the ref to react-hook-form
                                    (
                                      fileInputRef as React.MutableRefObject<HTMLInputElement | null>
                                    ).current = e; // Assign to your local ref
                                  }}
                                  onChange={(event) => {
                                    const file = event.target.files?.[0];
                                    if (file) {
                                      setFile(file);
                                      onChange(file);
                                    }
                                  }}
                                />
                                <p className="text-sm text-gray-500 mb-4">
                                  File up to 50MB
                                </p>
                                <p className="font-medium mb-1">
                                  {file ? file.name : "Upload Component WASM"}
                                </p>
                              </div>
                            </div>
                          </FormControl>
                        </FormItem>
                      )}
                    />
                    <div className="flex justify-end">
                      <Button type="submit">Update</Button>
                    </div>
                  </form>
                </Form>
              </CardContent>
            </Card>
          </div>
        </div>
      </div>
    </ErrorBoundary>
  );
}
