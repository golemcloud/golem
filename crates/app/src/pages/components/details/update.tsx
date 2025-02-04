import { useParams } from "react-router-dom";
import { useRef, useState } from "react";
import { DndProvider } from "react-dnd";
import { HTML5Backend } from "react-dnd-html5-backend";
import {
  Card,
  CardContent,
  CardDescription,
  CardTitle,
} from "@/components/ui/card";
import {
  Form,
  FormControl,
  FormField,
  FormItem,
  FormLabel,
} from "@/components/ui/form";
import { Input } from "@/components/ui/input";
import { FileUp } from "lucide-react";
import { Button } from "@/components/ui/button";
import { useForm } from "react-hook-form";
import { z } from "zod";
import { zodResolver } from "@hookform/resolvers/zod";
import { API } from "@/service";
import { toast } from "@/hooks/use-toast";
import { FileManager } from "../create/fileManager";

/**
 * Example Zod schema that checks:
 * - File instance
 * - File size < 50MB
 * - (Optional) Basic file extension check for .wasm
 */
const formSchema = z.object({
  component: z
    .instanceof(File)
    .refine((file) => file.size < 50_000_000, {
      message: "Your file must be less than 50MB.",
    })
    .refine((file) => file.name.toLowerCase().endsWith(".wasm"), {
      message: "Only .wasm files are allowed.",
    }),
});

export default function ComponentUpdate() {
  const { componentId } = useParams();
  const [file, setFile] = useState<File | null>(null);

  // Reference for manually triggering the file input
  const fileInputRef = useRef<HTMLInputElement>(null);

  /**
   * react-hook-form setup
   */
  const form = useForm<z.infer<typeof formSchema>>({
    resolver: zodResolver(formSchema),
    defaultValues: {
      component: undefined,
    },
  });

  /**
   * Form submit handler
   */
  async function onSubmit() {
    if (!file) {
      toast({
        title: "No file selected",
        description: "Please select a .wasm file before updating.",
        variant: "destructive",
      });
      return;
    }

    try {
      const formData = new FormData();
      formData.append("component", file);
      await API.updateComponent(componentId!, formData);

      form.reset();
      setFile(null);

      toast({
        title: "Component was updated successfully",
        duration: 3000,
      });
    } catch (err) {
      console.error("Error updating component:", err);
      toast({
        title: "Failed to update component",
        description: String(err),
        variant: "destructive",
      });
    }
  }

  return (
    <div className="flex">
      <div className="flex-1 p-8">
        <Card className="max-w-4xl mx-auto border-0 shadow-none">
          <CardTitle>
            <h1 className="text-2xl font-semibold mb-1">Update Component</h1>
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
                {/* COMPONENT FILE */}
                <FormField
                  control={form.control}
                  name="component"
                  render={({ field: { onChange, onBlur, name, ref } }) => (
                    <FormItem>
                      <FormLabel>Component</FormLabel>
                      <FormControl>
                        <div
                          className="border-2 border-dashed border-gray-200 rounded-lg p-8 cursor-pointer hover:border-gray-400"
                          onClick={() => fileInputRef.current?.click()}
                        >
                          <div className="flex flex-col items-center justify-center text-center">
                            <FileUp className="h-8 w-8 text-gray-400 mb-3" />
                            <Input
                              type="file"
                              accept=".wasm"
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
                                const selectedFile = event.target.files?.[0];
                                if (selectedFile) {
                                  setFile(selectedFile);
                                  onChange(selectedFile);
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

                {/* DRAG & DROP FILE MANAGER */}
                <DndProvider backend={HTML5Backend}>
                  <FileManager />
                </DndProvider>

                <div className="flex justify-end">
                  <Button type="submit">Update</Button>
                </div>
              </form>
            </Form>
          </CardContent>
        </Card>
      </div>
    </div>
  );
}
