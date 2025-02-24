import { useParams, useNavigate } from "react-router-dom";
import { useRef, useState } from "react";
import { DndProvider } from "react-dnd";
import JSZip from "jszip";
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
import { FileManager, FileItem } from "../create/fileManager";

const formSchema = z.object({
  component: z
    .instanceof(File)
    .refine(file => file.size < 50_000_000, {
      message: "Your file must be less than 50MB.",
    })
    .refine(file => file.name.toLowerCase().endsWith(".wasm"), {
      message: "Only .wasm files are allowed.",
    }),
});

export default function ComponentUpdate() {
  const { componentId } = useParams();
  const navigate = useNavigate();
  const [fileSystem, setFileSystem] = useState<FileItem[] | []>([]);
  const [file, setFile] = useState<File | null>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);

  const form = useForm<z.infer<typeof formSchema>>({
    resolver: zodResolver(formSchema),
    defaultValues: { component: undefined },
  });

  async function addFilesToZip(zipFolder: JSZip, parentId: string | null) {
    const children = fileSystem.filter(file => file.parentId === parentId);
    for (const child of children) {
      if (child.type === "folder") {
        const folder = zipFolder.folder(child.name);
        if (folder) {
          await addFilesToZip(folder, child.id);
        }
      } else if (child.type === "file") {
        if (child.fileObject) {
          zipFolder.file(child.name, child.fileObject);
        }
      }
    }
  }

  // Recursive helper function to compute full path of a file item.
  function getFullPath(file: FileItem, allFiles: FileItem[]): string {
    if (!file.parentId) return `/${file.name}`;
    const parent = allFiles.find(f => f.id === file.parentId);
    if (!parent) return file.name;
    return `${getFullPath(parent, allFiles)}/${file.name}`;
  }

  function captureFileMetadata(allFiles: FileItem[]) {
    const filesPath: { path: string; permissions: string }[] = [];
    allFiles.forEach(file => {
      if (file.type != "folder") {
        filesPath.push({
          path: getFullPath(file, allFiles),
          permissions: file.isLocked ? "read-only" : "read-write",
        });
      }
    });
    return { values: filesPath };
  }
  async function onSubmit() {
    if (!file) {
      toast({
        title: "No file selected",
        description: "Please select a .wasm file.",
        variant: "destructive",
      });
      return;
    }

    try {
      const formData = new FormData();
      formData.append("component", file);
      const zip = new JSZip();
      await addFilesToZip(zip, null);
      const blob = await zip.generateAsync({ type: "blob" });
      formData.append(
        "filesPermissions",
        JSON.stringify(captureFileMetadata(fileSystem)),
      );
      formData.append("files", blob, "temp.zip");
      await API.updateComponent(componentId!, formData);

      form.reset();
      setFile(null);
      setFileSystem([]);

      toast({ title: "Component updated successfully", duration: 3000 });
      navigate(`/components/${componentId}`);
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
    <div className="flex justify-center px-6 py-10">
      <Card className="max-w-3xl w-full border border-gray-200 shadow-lg rounded-lg p-6">
        <CardTitle className="text-2xl font-semibold mb-2">
          Update Component
        </CardTitle>
        <CardDescription className="text-gray-600">
          Modify and update your component below.
        </CardDescription>
        <CardContent className="p-6">
          <Form {...form}>
            <form onSubmit={form.handleSubmit(onSubmit)} className="space-y-6">
              <FormField
                control={form.control}
                name="component"
                render={({ field: { onChange, onBlur, name, ref } }) => (
                  <FormItem>
                    <FormLabel className="text-gray-700 font-medium">
                      Component File
                    </FormLabel>
                    <FormControl>
                      <div
                        className="border-2 border-dashed border-gray-300 rounded-lg p-6 text-center cursor-pointer hover:border-gray-500"
                        onClick={() => fileInputRef.current?.click()}
                      >
                        <FileUp className="h-10 w-10 text-gray-400 mx-auto mb-3" />
                        <Input
                          type="file"
                          accept=".wasm"
                          className="hidden"
                          name={name}
                          onBlur={onBlur}
                          ref={e => {
                            ref(e); // Forward the ref to react-hook-form
                            (
                              fileInputRef as React.MutableRefObject<HTMLInputElement | null>
                            ).current = e; // Assign to your local ref
                          }}
                          onChange={event => {
                            const selectedFile = event.target.files?.[0];
                            if (selectedFile) {
                              setFile(selectedFile);
                              onChange(selectedFile);
                            }
                          }}
                        />
                        <p className="text-gray-500">Max file size: 50MB</p>
                        <p className="font-medium text-gray-400 mt-2">
                          {file ? file.name : "Upload WASM File"}
                        </p>
                      </div>
                    </FormControl>
                  </FormItem>
                )}
              />
              <DndProvider backend={HTML5Backend}>
                <FileManager files={fileSystem} setFiles={setFileSystem} />
              </DndProvider>
              <div className="flex justify-end">
                <Button type="submit" className="px-6 py-2">
                  Update
                </Button>
              </div>
            </form>
          </Form>
        </CardContent>
      </Card>
    </div>
  );
}
