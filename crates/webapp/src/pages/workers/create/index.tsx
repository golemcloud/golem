import {useEffect, useState} from 'react'
import {Button} from "@/components/ui/button"
import {Card, CardContent, CardDescription, CardTitle} from "@/components/ui/card"
import {Input} from "@/components/ui/input"
import {v4 as uuidv4} from 'uuid';
import {Select, SelectContent, SelectItem, SelectTrigger, SelectValue} from "@/components/ui/select"
import {z} from "zod"
import {useFieldArray, useForm} from "react-hook-form";
import {zodResolver} from "@hookform/resolvers/zod";
import {Form, FormControl, FormDescription, FormField, FormItem, FormLabel, FormMessage,} from "@/components/ui/form"
import {API} from "@/service";
import {Component} from "@/types/component.ts";

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
})

export default function CreateWorker() {
    const form = useForm<z.infer<typeof formSchema>>({
        resolver: zodResolver(formSchema),
        defaultValues: {
            componentID: "",
            name: "",
            env: [{key: "", value: ""}],
            args: [""],
        },
    });

    const {fields: envFields, append: appendEnv, remove: removeEnv} = useFieldArray({
        control: form.control,
        name: "env",
    });

    const {fields: argFields, append: appendArg, remove: removeArg} = useFieldArray({
        control: form.control,
        name: "args",
    });

    function generateUUID() {
        form.setValue("name", uuidv4());
    }

    function onSubmit(values: z.infer<typeof formSchema>) {
        const {componentID, ...rest} = values as any;
        console.log("submit", rest);
        rest.env = rest.env.reduce((acc: Record<string, string>, arg: { key: string, value: string }) => {
            if (arg.key) {
                acc[arg.key] = arg.value;
            }
            return acc;
        }, {});
        rest.args = rest.args.filter((x) => x && x.length > 0);

        console.log("submit before", rest);
        API.createWorker(componentID, rest).then((response) => {
            console.log(response);
        });
    }

    const [components, setComponents] = useState<{ [key: string]: Component }>({});

    useEffect(() => {
        API.getComponentByIdAsKey().then((response) => setComponents(response));
    }, []);

    return (
        <div className="p-4 min-h-screen bg-background text-foreground mx-auto max-w-7xl px-6 lg:px-8 py-4">
            <Card className="max-w-2xl mx-auto border-0 shadow-none">
                <CardTitle><h1 className="text-2xl font-semibold mb-1">Create a new Worker</h1></CardTitle>
                <CardDescription><p className="text-sm text-gray-400">Launch a new worker</p></CardDescription>
                <CardContent className="p-6">

                    <Form {...form}>
                        <form onSubmit={form.handleSubmit(onSubmit)} className="space-y-8">
                            <FormField
                                control={form.control}
                                name="componentID"
                                render={({field}) => (
                                    <FormItem>
                                        <FormLabel>Component</FormLabel>
                                        <FormControl>
                                            <Select onValueChange={field.onChange}
                                                    defaultValue={field.value}
                                                    {...field}>
                                                <SelectTrigger id="componentID">
                                                    <SelectValue placeholder="choose a component"/>
                                                </SelectTrigger>
                                                <SelectContent>
                                                    {Object.values(components).map((data: Component) => (
                                                        <SelectItem value={data.componentId!}>
                                                            {data?.componentName}
                                                        </SelectItem>
                                                    ))}
                                                </SelectContent>
                                            </Select>
                                        </FormControl>
                                        <FormMessage/>
                                    </FormItem>
                                )}
                            />
                            <FormField
                                control={form.control}
                                name="name"
                                render={({field}) => (
                                    <FormItem>
                                        <FormLabel>Worker Name</FormLabel>
                                        <FormControl>
                                            <div className="flex gap-2">
                                                <Input {...field} />
                                                <Button variant="secondary" onClick={generateUUID}>
                                                    Generate
                                                </Button>
                                            </div>
                                        </FormControl>
                                        <FormDescription>The name must be unique for this component.</FormDescription>
                                        <FormMessage/>
                                    </FormItem>
                                )}
                            />
                            <div>
                                <FormLabel>Environment Variables</FormLabel>
                                {envFields.map((field, index) => (
                                    <div key={field.id} className="flex items-center gap-2 pt-2">
                                        <FormField
                                            control={form.control}
                                            name={`env.${index}.key`}
                                            render={({field}) => (
                                                <FormControl>
                                                    <Input placeholder="Key" {...field} />
                                                </FormControl>
                                            )}
                                        />
                                        <FormField
                                            control={form.control}
                                            name={`env.${index}.value`}
                                            render={({field}) => (
                                                <FormControl>
                                                    <Input placeholder="Value" {...field} />
                                                </FormControl>
                                            )}
                                        />
                                        <Button type="button" variant="secondary" size="sm"
                                                onClick={() => removeEnv(index)}>
                                            Remove
                                        </Button>
                                    </div>
                                ))}
                                <Button className={"mt-4"} type="button" variant="outline" size="sm"
                                        onClick={() => appendEnv({key: "", value: ""})}
                                        disabled={envFields.some((field) => !field.key && !field.value)}>
                                    Add Environment Variable
                                </Button>
                            </div>
                            <div>
                                <div className="flex items-center gap-2 pb-2 w-full">
                                    <FormLabel>Arguments</FormLabel>
                                    <Button type="button" variant="secondary" size="sm" onClick={() => appendArg("")}>
                                        Add
                                    </Button>
                                </div>

                                {argFields.map((field, index) => (
                                    <div key={field.id} className="flex items-center gap-2 pb-2">
                                        <FormField
                                            control={form.control}
                                            name={`args.${index}`}
                                            render={({field}) => (
                                                <FormControl>
                                                    <Input {...field} />
                                                </FormControl>
                                            )}
                                        />
                                        <Button type="button" variant="secondary" size="sm"
                                                onClick={() => removeArg(index)}>
                                            Remove
                                        </Button>
                                    </div>
                                ))}
                            </div>
                            <div className="flex justify-end">
                                <Button type="submit">Submit</Button>
                            </div>

                        </form>
                    </Form>
                </CardContent>
            </Card>
        </div>
    )
}

