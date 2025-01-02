import React from "react";
import {Card, CardContent, CardDescription, CardTitle} from "@/components/ui/card.tsx";
import {z} from "zod";
import {Form, useForm} from "react-hook-form";
import {zodResolver} from "@hookform/resolvers/zod";
import {FormControl, FormDescription, FormField, FormItem, FormLabel, FormMessage} from "@/components/ui/form.tsx";
import {Input} from "@/components/ui/input.tsx";
import {Label} from "@radix-ui/react-label";
import {RadioGroup, RadioGroupItem} from "@/components/ui/radio-group.tsx";


const formSchema = z.object({
    name: z.string().min(4, {
        message: "Component name must be at least 4 characters",
    }),
    type: z.enum(["durable", "ephemeral"]),
    component: z.instanceof(File)
})


const CreateComponent = () => {
    // const navigate = useNavigate();
    const form = useForm<z.infer<typeof formSchema>>({
        resolver: zodResolver(formSchema),
        defaultValues: {
            name: "",
            type: undefined,
            component: undefined,
        },
    });
    // const [componentName, setComponentName] = useState("");
    // const [type, setType] = useState<"durable" | "ephemeral">("durable");
    // const [file, setFile] = useState<File | null>(null);
    // const fileInputRef = useRef<HTMLInputElement>(null);

    function onSubmit(values: z.infer<typeof formSchema>) {
        console.log("submit", values);
    }


    const handleFileSelect = (e: React.ChangeEvent<HTMLInputElement>) => {
        // const selectedFile = e.target.files?.[0];
        // if (selectedFile && selectedFile.size <= 50 * 1024 * 1024) {
        //     // 50MB limit
        //     setFile(selectedFile);
        //     console.log("selectedFile : ", selectedFile)
        //     // Prepare FormData
        //     const formData = new FormData();
        //     formData.append("name", "newVasanth"); // Append the file
        //     formData.append("component", selectedFile); // Append the file
        //     formData.append("componentType", "Durable"); // Add file name
        //     // formData.append("type", type); // Add type
        //
        //     // callFormDataApi(selectedFile.name, selectedFile.arrayBuffer(), selectedFile.name).then((_x)=>console.log("api done"));
        //     // callFormDataApi(formData).then((_x) => console.log("api done"));
        // }
    };

    return (
        <div className="p-4 min-h-screen bg-background text-foreground mx-auto max-w-7xl px-6 lg:px-8 py-4">
            <Card className="max-w-2xl mx-auto border-0 shadow-none">
                <CardTitle><h1 className="text-2xl font-semibold mb-1">Create a new Component</h1></CardTitle>
                <CardDescription><p className="text-sm text-gray-400">Components are the building blocks</p>
                </CardDescription>
                <CardContent className="p-6">
                    <Form {...form}>
                        <form onSubmit={form.handleSubmit(onSubmit)} className="space-y-8">
                            <FormField
                                control={form.control}
                                name="name"
                                render={({field}) => (
                                    <FormItem>
                                        <FormLabel>Component</FormLabel>
                                        <FormControl>
                                            <Input {...field} />
                                        </FormControl>
                                        <FormMessage/>
                                    </FormItem>
                                )}
                            />
                            <FormField
                                control={form.control}
                                name="type"
                                render={({field}) => (
                                    <FormItem>
                                        <FormLabel>Worker Name</FormLabel>
                                        <FormControl>
                                            <RadioGroup {...field}>
                                                <div className="flex items-center space-x-2">
                                                    <RadioGroupItem value="default" id="r1"/>
                                                    <Label htmlFor="r1">Default</Label>
                                                </div>
                                                <div className="flex items-center space-x-2">
                                                    <RadioGroupItem value="comfortable" id="r2"/>
                                                    <Label htmlFor="r2">Comfortable</Label>
                                                </div>
                                                <div className="flex items-center space-x-2">
                                                    <RadioGroupItem value="compact" id="r3"/>
                                                    <Label htmlFor="r3">Compact</Label>
                                                </div>
                                            </RadioGroup>
                                        </FormControl>
                                        <FormDescription>The name must be unique for this component.</FormDescription>
                                        <FormMessage/>
                                    </FormItem>
                                )}
                            />
                        </form>
                    </Form>
                </CardContent>

            </Card>
            {/*<h1 className="text-2xl font-semibold mb-2">Create a new Component</h1>*/}
            {/*<p className="text-gray-600 mb-8">*/}
            {/*    Components are the building blocks for your project*/}
            {/*</p>*/}

            {/*<div className="space-y-8">*/}
            {/*    /!* Project and Component Name *!/*/}
            {/*    <div className="grid">*/}
            {/*        <div className="col-span-2">*/}
            {/*            <label className="block text-sm font-medium text-gray-700 mb-1">*/}
            {/*                Component Name*/}
            {/*            </label>*/}
            {/*            <div className="flex items-center">*/}
            {/*                <input*/}
            {/*                    type="text"*/}
            {/*                    value={componentName}*/}
            {/*                    onChange={(e) => setComponentName(e.target.value)}*/}
            {/*                    className="flex-1 border border-gray-200 rounded px-3 py-2 focus:outline-none focus:ring-2 focus:ring-blue-500"*/}
            {/*                    placeholder="Enter component name"*/}
            {/*                />*/}
            {/*            </div>*/}
            {/*        </div>*/}
            {/*    </div>*/}

            {/*    /!* Type Selection *!/*/}
            {/*    <div>*/}
            {/*        <label className="block text-sm font-medium text-gray-700 mb-3">*/}
            {/*            Type*/}
            {/*        </label>*/}
            {/*        <div className="space-y-3">*/}
            {/*            <label*/}
            {/*                className="flex items-start space-x-3 p-3 border border-gray-200 rounded cursor-pointer hover:bg-gray-50">*/}
            {/*                <input*/}
            {/*                    type="radio"*/}
            {/*                    name="type"*/}
            {/*                    value="durable"*/}
            {/*                    checked={type === "durable"}*/}
            {/*                    onChange={() => setType("durable")}*/}
            {/*                    className="mt-1"*/}
            {/*                />*/}
            {/*                <div>*/}
            {/*                    <div className="flex items-center space-x-2">*/}
            {/*                        <Database className="h-5 w-5 text-gray-600"/>*/}
            {/*                        <span className="font-medium">Durable</span>*/}
            {/*                    </div>*/}
            {/*                    <p className="text-sm text-gray-600 mt-1">*/}
            {/*                        Workers are persistent and executed with transactional*/}
            {/*                        guarantees*/}
            {/*                        <br/>*/}
            {/*                        Ideal for stateful and high-reliability use cases*/}
            {/*                    </p>*/}
            {/*                </div>*/}
            {/*            </label>*/}
            {/*            <label*/}
            {/*                className="flex items-start space-x-3 p-3 border border-gray-200 rounded cursor-pointer hover:bg-gray-50">*/}
            {/*                <input*/}
            {/*                    type="radio"*/}
            {/*                    name="type"*/}
            {/*                    value="ephemeral"*/}
            {/*                    checked={type === "ephemeral"}*/}
            {/*                    onChange={() => setType("ephemeral")}*/}
            {/*                    className="mt-1"*/}
            {/*                />*/}
            {/*                <div>*/}
            {/*                    <div className="flex items-center space-x-2">*/}
            {/*                        <Zap className="h-5 w-5 text-gray-600"/>*/}
            {/*                        <span className="font-medium">Ephemeral</span>*/}
            {/*                    </div>*/}
            {/*                    <p className="text-sm text-gray-600 mt-1">*/}
            {/*                        Workers are transient and executed normally*/}
            {/*                        <br/>*/}
            {/*                        Ideal for stateless and low-reliability use cases*/}
            {/*                    </p>*/}
            {/*                </div>*/}
            {/*            </label>*/}
            {/*        </div>*/}
            {/*    </div>*/}

            {/*    <WasmUpload/>*/}
            {/*    <FileManager/>*/}
            {/*    <div*/}
            {/*        className="border-2 border-dashed border-gray-200 rounded-lg p-8 cursor-pointer hover:border-gray-400"*/}
            {/*        onClick={() => fileInputRef.current?.click()}*/}
            {/*    >*/}
            {/*        File Manager*/}
            {/*        <p className="font-medium mb-1">*/}
            {/*            {file ? file.name : "Upload Component WASM"}*/}
            {/*        </p>*/}
            {/*        <input*/}
            {/*            ref={fileInputRef}*/}
            {/*            type="file"*/}
            {/*            accept="application/wasm,.wasm"*/}
            {/*            onChange={handleFileSelect}*/}
            {/*            className="hidden"*/}
            {/*        />*/}
            {/*    </div>*/}

            {/*    <div className="flex justify-end">*/}
            {/*        <Button>*/}
            {/*            <PlusCircle className="mr-2 size-4"/>*/}
            {/*            Create Component*/}
            {/*        </Button>*/}
            {/*    </div>*/}
            {/*</div>*/}
        </div>
    );
};

export default CreateComponent;
