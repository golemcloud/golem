import {Button} from "@/components/ui/button";
import {Card, CardContent, CardTitle,} from "@/components/ui/card";
import {Input} from "@/components/ui/input";
import {z} from "zod";
import {useForm} from "react-hook-form";
import {zodResolver} from "@hookform/resolvers/zod";
import {Form, FormControl, FormDescription, FormField, FormItem, FormLabel, FormMessage,} from "@/components/ui/form";
import {Textarea} from "@/components/ui/textarea.tsx";

const formSchema = z.object({
    name: z.string().min(2, {
        message: "Plugin name must be at least 2 characters.",
    }),
    version: z.string().regex(/^\d+\.\d+\.\d+$/, {
        message: "Version must be in the format x.y.z",
    }),
    description: z.string().min(10, {
        message: "Description must be at least 10 characters.",
    }),
    icon: z.array(z.instanceof(File).refine((file) => file.size < 5000000, {
        message: "Your Icon must be less than 5MB.",
    })),
    homepage: z.string().url({
        message: "Please enter a valid URL.",
    }),
    specs: z.object({
        type: z.string().default("ComponentTransformer"),
    }),
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
            icon: [],
            homepage: "",
            specs: {
                type: "ComponentTransformer",
            },
            scope: {
                type: "Global",
            },
        },
    });


    function onSubmit(values: z.infer<typeof formSchema>) {
        console.log("submit", values);
    }

    return (
        <div className="p-4 min-h-screen bg-background text-foreground mx-auto max-w-7xl px-6 lg:px-8 py-4">
            <Card className="max-w-2xl mx-auto border-0 shadow-none">
                <CardTitle>
                    <h1 className="text-2xl font-semibold mb-1">Create a new Plugin</h1>
                </CardTitle>
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
                                            <Input {...field} placeholder="Component name"/>
                                        </FormControl>
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
                                            <Input {...field} placeholder="0.1.0"/>
                                        </FormControl>
                                        <FormDescription>Version of the component to be deployed</FormDescription>
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
                                            <Textarea {...field} placeholder="..."/>
                                        </FormControl>
                                        <FormDescription>Version of the component to be deployed</FormDescription>
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
                                            <Input {...field} placeholder="https://homepage.com"/>
                                        </FormControl>
                                        <FormMessage/>
                                    </FormItem>
                                )}
                            />
                            <div className="flex justify-end">
                                <Button type="submit">Submit</Button>
                            </div>
                        </form>
                    </Form>
                </CardContent>
            </Card>
        </div>
    );
}
