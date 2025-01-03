import {Button} from "@/components/ui/button";
import {Card, CardContent, CardDescription, CardTitle,} from "@/components/ui/card";
import {Input} from "@/components/ui/input";
import {z} from "zod";
import {useForm} from "react-hook-form";
import {zodResolver} from "@hookform/resolvers/zod";
import {Form, FormControl, FormDescription, FormField, FormItem, FormLabel, FormMessage,} from "@/components/ui/form";
import {Textarea} from "@/components/ui/textarea.tsx";
import {RadioGroup, RadioGroupItem} from "@/components/ui/radio-group.tsx";
import {Select, SelectContent, SelectItem, SelectTrigger, SelectValue} from "@/components/ui/select.tsx";

const OplogProcessorSpecSchema = z.object({
    type: z.literal('OplogProcessor'),
    componentId: z.string().uuid(),
    componentVersion: z.number().int(),
});

const ComponentTransformerSpecSchema = z.object({
    type: z.literal('ComponentTransformer'),
    jsonSchema: z.string()
    //     .refine((val) => {
    //     try {
    //         JSON.parse(val);
    //         return true;
    //     } catch (error) {
    //         return false;
    //     }
    // }, {
    //     message: "Invalid JSON structure",
    // }),
    ,
    validateUrl: z.string().url(),
    transformUrl: z.string().url(),
});

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
    icon: z.instanceof(File),
    homepage: z.string().url({
        message: "Please enter a valid URL.",
    }),
    validateURL: z.string().url({
        message: "Please enter a valid URL.",
    }),
    transformURL: z.string().url({
        message: "Please enter a valid URL.",
    }),
    specs: z.discriminatedUnion("type", [
        OplogProcessorSpecSchema,
        ComponentTransformerSpecSchema
    ]),
    scope: z.object({
        type: z.string().default("Global"),
    }),
})

export default function CreatePlugin() {
    const form = useForm<z.infer<typeof formSchema>>({
        resolver: zodResolver(formSchema),
        defaultValues: {
            name: "",
            version: "",
            description: "",
            homepage: "",
            validateURL: "",
            transformURL: "",
            specs: {
                type: "OplogProcessor"
            },
            scope: {
                type: "Global",
            },
        },
    });


    function onSubmit(values: z.infer<typeof formSchema>) {
        console.log("submit")
        // console.log("submit", values);
    }

    return (
        <div className="p-4 min-h-screen bg-background text-foreground mx-auto max-w-7xl px-6 lg:px-8 py-4">
            <Card className="max-w-4xl mx-auto border-0 shadow-none">
                <CardTitle>
                    <h1 className="text-2xl font-semibold mb-1">Create a new Plugin</h1>
                </CardTitle>
                <CardDescription>
                    <p className="text-sm text-gray-400">Start a new plugin</p>
                </CardDescription>
                <CardContent className="p-6">
                    <Form {...form}>
                        <form onSubmit={form.handleSubmit(onSubmit)} className="space-y-8">
                            <FormField
                                control={form.control}
                                name="name"
                                render={({field}) => (
                                    <FormItem>
                                        <FormLabel>Name</FormLabel>
                                        <FormControl>
                                            <Input placeholder="Plugin name" {...field} />
                                        </FormControl>
                                        <FormDescription>
                                            Enter the name of your plugin.
                                        </FormDescription>
                                        <FormMessage/>
                                    </FormItem>
                                )}
                            />
                            <FormField
                                control={form.control}
                                name="version"
                                render={({field}) => (
                                    <FormItem>
                                        <FormLabel>Version</FormLabel>
                                        <FormControl>
                                            <Input placeholder="v0" {...field} />
                                        </FormControl>
                                        <FormDescription>
                                            Enter the version in the format v0.
                                        </FormDescription>
                                        <FormMessage/>
                                    </FormItem>
                                )}
                            />
                            <FormField
                                control={form.control}
                                name="description"
                                render={({field}) => (
                                    <FormItem>
                                        <FormLabel>Description</FormLabel>
                                        <FormControl>
                                            <Textarea placeholder="Describe your plugin" {...field} />
                                        </FormControl>
                                        <FormDescription>
                                            Provide a brief description of your plugin.
                                        </FormDescription>
                                        <FormMessage/>
                                    </FormItem>
                                )}
                            />
                            <FormField
                                control={form.control}
                                name="icon"
                                render={({field: {onChange, value, ...field}}) => (
                                    <FormItem>
                                        <FormLabel>Icon</FormLabel>
                                        <FormControl>
                                            <Input
                                                type="file"
                                                accept="image/*"
                                                onChange={(e) => {
                                                    const file = e.target.files?.[0]
                                                    if (file) onChange(file)
                                                }}
                                                {...field}
                                            />
                                        </FormControl>
                                        <FormDescription>
                                            Upload an icon for your plugin.
                                        </FormDescription>
                                        <FormMessage/>
                                    </FormItem>
                                )}
                            />
                            <FormField
                                control={form.control}
                                name="homepage"
                                render={({field}) => (
                                    <FormItem>
                                        <FormLabel>Homepage</FormLabel>
                                        <FormControl>
                                            <Input placeholder="https://example.com" {...field} />
                                        </FormControl>
                                        <FormDescription>
                                            Enter the homepage URL for your plugin.
                                        </FormDescription>
                                        <FormMessage/>
                                    </FormItem>
                                )}
                            />

                            <FormField
                                control={form.control}
                                name="specs.type"
                                render={({field}) => (
                                    <FormItem className="space-y-3">
                                        <FormLabel>Specs Type</FormLabel>
                                        <FormControl>
                                            <RadioGroup
                                                onValueChange={field.onChange}
                                                defaultValue={field.value}
                                                className="flex flex-col space-y-1"
                                            >
                                                <FormItem className="flex items-center space-x-3 space-y-0">
                                                    <FormControl>
                                                        <RadioGroupItem value="OplogProcessor"/>
                                                    </FormControl>
                                                    <FormLabel className="font-normal">
                                                        OplogProcessor
                                                    </FormLabel>
                                                </FormItem>
                                                <FormItem className="flex items-center space-x-3 space-y-0">
                                                    <FormControl>
                                                        <RadioGroupItem value="ComponentTransformer"/>
                                                    </FormControl>
                                                    <FormLabel className="font-normal">
                                                        ComponentTransformer
                                                    </FormLabel>
                                                </FormItem>
                                            </RadioGroup>
                                        </FormControl>
                                        <FormMessage/>
                                    </FormItem>
                                )}
                            />
                            {form.watch("specs.type") === "OplogProcessor" && (
                                <>
                                    <FormField
                                        control={form.control}
                                        name="specs.componentId"
                                        render={({field}) => (
                                            <FormItem>
                                                <FormLabel>Component ID</FormLabel>
                                                <FormControl>
                                                    <Input {...field} />
                                                </FormControl>
                                                <FormMessage/>
                                            </FormItem>
                                        )}
                                    />
                                    <FormField
                                        control={form.control}
                                        name="specs.componentVersion"
                                        render={({field}) => (
                                            <FormItem>
                                                <FormLabel>Component Version</FormLabel>
                                                <FormControl>
                                                    <Input type="number" {...field}
                                                           onChange={e => field.onChange(parseInt(e.target.value))}/>
                                                </FormControl>
                                                <FormMessage/>
                                            </FormItem>
                                        )}
                                    />
                                </>
                            )}
                            {form.watch("specs.type") === "ComponentTransformer" && (
                                <>
                                    <FormField
                                        control={form.control}
                                        name="validateURL"
                                        render={({field}) => (
                                            <FormItem>
                                                <FormLabel>Validate URL</FormLabel>
                                                <FormControl>
                                                    <Input placeholder="https://api.example.com/validate" {...field} />
                                                </FormControl>
                                                <FormDescription>
                                                    Enter the URL for validating your plugin.
                                                </FormDescription>
                                                <FormMessage/>
                                            </FormItem>
                                        )}
                                    />
                                    <FormField
                                        control={form.control}
                                        name="transformURL"
                                        render={({field}) => (
                                            <FormItem>
                                                <FormLabel>Transform URL</FormLabel>
                                                <FormControl>
                                                    <Input placeholder="https://api.example.com/transform" {...field} />
                                                </FormControl>
                                                <FormDescription>
                                                    Enter the URL for transforming your plugin.
                                                </FormDescription>
                                                <FormMessage/>
                                            </FormItem>
                                        )}
                                    />
                                    <FormField
                                        control={form.control}
                                        name="specs.jsonSchema"
                                        render={({field}) => (
                                            <FormItem>
                                                <FormLabel>JSON Schema</FormLabel>
                                                <FormControl>
                                                    <Textarea {...field} placeholder="Enter your JSON schema here..."/>
                                                </FormControl>
                                                <FormDescription>
                                                    Enter a valid JSON schema.
                                                </FormDescription>
                                                <FormMessage/>
                                            </FormItem>
                                        )}
                                    />
                                </>
                            )}
                            <FormField
                                control={form.control}
                                name="scope.type"
                                render={({field}) => (
                                    <FormItem>
                                        <FormLabel>Scope Type</FormLabel>
                                        <Select onValueChange={field.onChange} defaultValue={field.value} {...field}>
                                            <FormControl>
                                                <SelectTrigger>
                                                    <SelectValue placeholder="Select a scope type"/>
                                                </SelectTrigger>
                                            </FormControl>
                                            <SelectContent>
                                                <SelectItem value="Global">Global</SelectItem>
                                                <SelectItem value="Local">Local</SelectItem>
                                            </SelectContent>
                                        </Select>
                                        <FormMessage/>
                                    </FormItem>
                                )}
                            />

                            <div className="flex justify-end">
                                <Button type="submit">Create Plugin</Button>
                            </div>
                        </form>
                    </Form>
                </CardContent>
            </Card>
        </div>
    );
}
