// @ts-nocheck
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardTitle,
} from "@/components/ui/card";
import { Input } from "@/components/ui/input";
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
import { ArrowLeft } from "lucide-react";
import { useNavigate, useParams } from "react-router-dom";

const formSchema = z.object({
  componentID: z.string(),
  name: z.string().min(4, {
    message: "Worker name must be at least 4 characters",
  }),
  env: z.array(
    z.object({
      key: z.string(),
      value: z.string(),
    }),
  ),
  args: z.array(z.string()),
});

export default function CreateWorker() {
  const navigate = useNavigate();
  const { componentId, appId } = useParams();
  const form = useForm({
    resolver: zodResolver(formSchema),
    defaultValues: {
      componentID: componentId,
      name: "",
      env: [{ key: "", value: "" }],
      args: [""],
    },
  });

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

  function onSubmit(values) {
    const { componentID, ...rest } = values;
    rest.env = rest.env.reduce((acc, arg) => {
      if (arg.key) acc[arg.key] = arg.value;
      return acc;
    }, {});
    rest.args = rest.args.filter(x => x.trim().length > 0);

    API.workerService
      .createWorker(appId, componentID, values.name)
      .then((response: { component_name: string; worker_name: string }) => {
        navigate(
          `/app/${appId}/components/${componentId}/workers/${response.worker_name}`,
        );
      });
  }

  return (
    <div className="flex justify-center p-10">
      <Card className="w-full max-w-2xl border shadow-md p-6">
        <CardTitle className="text-xl font-bold">Create a New Worker</CardTitle>
        <CardDescription className="text-gray-500 mb-6">
          Launch a new worker with the required settings.
        </CardDescription>
        <CardContent>
          <Form {...form}>
            <form onSubmit={form.handleSubmit(onSubmit)} className="space-y-6">
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
                <div className="space-y-2 pt-2">
                  {envFields.map((field, index) => (
                    <div
                      key={field.name + field.componentId}
                      className="flex gap-2"
                    >
                      <Input
                        placeholder="Key"
                        {...form.register(`env.${index}.key`)}
                      />
                      <Input
                        placeholder="Value"
                        type="password"
                        {...form.register(`env.${index}.value`)}
                      />
                      <Button
                        type="button"
                        variant="destructive"
                        size="sm"
                        onClick={() => removeEnv(index)}
                      >
                        Remove
                      </Button>
                    </div>
                  ))}
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    onClick={() => appendEnv({ key: "", value: "" })}
                  >
                    Add Environment Variable
                  </Button>
                </div>
              </div>

              <div>
                <FormLabel>Arguments</FormLabel>
                <div className="space-y-2 pt-2">
                  {argFields.map((field, index) => (
                    <div key={field.appId} className="flex gap-2">
                      <Input {...form.register(`args.${index}`)} />
                      <Button
                        type="button"
                        variant="destructive"
                        size="sm"
                        onClick={() => removeArg(index)}
                      >
                        Remove
                      </Button>
                    </div>
                  ))}
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    onClick={() => appendArg("")}
                  >
                    Add Argument
                  </Button>
                </div>
              </div>

              <div className="flex justify-between">
                <Button
                  type="button"
                  variant="secondary"
                  onClick={() => navigate(-1)}
                >
                  <ArrowLeft className="mr-2 h-5 w-5" /> Back
                </Button>
                <Button type="submit">Submit</Button>
              </div>
            </form>
          </Form>
        </CardContent>
      </Card>
    </div>
  );
}
