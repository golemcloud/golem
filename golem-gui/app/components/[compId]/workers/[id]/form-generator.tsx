import React, { useMemo } from "react";
import { useForm, Controller, Control, FieldErrors } from "react-hook-form";
import { TextField, Typography, Button, Stack } from "@mui/material";
import { Parameter } from "@/types/api";
import {getFormErrorMessage} from "@/lib/utils"

type FormData = {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  [key: string]: any; // Using `any` here for dynamic keys
};

const generateDefaultValues = (fields: Parameter[]): FormData => {
  const defaults: FormData = {};

  fields?.forEach((field) => {
    switch (field.typ.type) {
      case "Record":
        defaults[field.name] = generateDefaultValues(field.typ.fields || []);
        break;
      case "Tuple":
        defaults[field.name] = [generateDefaultValues(field.typ.items || [])];
        break;
      default:
        defaults[field.name] = "";
        break;
    }
  });

  return defaults;
};

const generateField = (
  field: Parameter,
  index: number,
  rootKey: string,
  control: Control<FormData>,
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  handleChange: (key: string, value: any) => void,
  errors: FieldErrors<FormData>
) => {
  const finalRootKey = `${rootKey ? `${rootKey}.` : ""}${field.name}`;
  // TODO need to add other types

  switch (field.typ.type) {
    case "Str":
      return (
        <>
          <Controller
            key={index}
            name={finalRootKey}
            control={control}
            rules={{ required: `${field.name} is Mandatory` }}
            render={({ field: _field }) => (
              <TextField
                {..._field}
                label={field.name}
                variant="outlined"
                fullWidth
                placeholder={field.name}
                className="mt-2"
                onChange={(e) => {
                  handleChange(finalRootKey, e.target.value);
                }}
              />
            )}
          />
          <Typography variant="caption" color="error">
            {getFormErrorMessage(finalRootKey, errors)}
          </Typography>
        </>
      );
    case "F32":
    case "F64":
    case "F16":
    case "U32":
    case "U64":
    case "U16":
      return (
        <>
          <Controller
            key={index}
            name={finalRootKey}
            rules={{ required: `${field.name} is Mandatory` }}
            control={control}
            render={({ field: _field }) => (
              <TextField
                {..._field}
                label={field.name}
                type="number"
                variant="outlined"
                className="mt-2"
                fullWidth
                placeholder={field.name}
                onChange={(e) => {
                  handleChange(
                    finalRootKey,
                    e.target.value ? Number(e.target.value) : ""
                  );
                }}
              />
            )}
          />

          <Typography variant="caption" color="error">
            {getFormErrorMessage(finalRootKey, errors)}
          </Typography>
        </>
      );
    case "Record":
      return (
        <>
          <Controller
            key={index}
            name={finalRootKey}
            rules={{ required: `${field.name} is Mandatory` }}
            control={control}
            render={({}) => (
              <div key={index}>
                <Typography variant="h6">{field.name}</Typography>
                {field?.typ?.fields?.map((nestedField, nestedIndex) =>
                  generateField(
                    nestedField,
                    nestedIndex,
                    finalRootKey,
                    control,
                    handleChange,
                    errors
                  )
                )}
              </div>
            )}
          />
        </>
      );
    case "Tuple":
      return (
        <Controller
          key={index}
          name={finalRootKey}
          control={control}
          render={({ field: { value } }) => {
            // eslint-disable-next-line @typescript-eslint/no-explicit-any
            const tuples = (value || [
              generateDefaultValues(field.typ.items || []),
            ]) as Array<any>;
            return (
              <>
                {tuples.map((tuple, idx) => (
                  <fieldset key={idx}>
                    <legend>
                      {field.name} (Tuple) {idx}
                    </legend>
                    <Button
                      type="button"
                      onClick={(e) => {
                        e.preventDefault();
                        const updatedTuples = [
                          ...tuples.slice(0, idx),
                          ...tuples.slice(idx + 1),
                        ];
                        handleChange(finalRootKey, updatedTuples);
                      }}
                    >
                      Remove Tuple
                    </Button>
                    {field.typ.items?.map((item, itemIdx) =>
                      generateField(
                        item,
                        itemIdx,
                        `${finalRootKey}[${idx}]`,
                        control,
                        handleChange,
                        errors
                      )
                    )}
                  </fieldset>
                ))}
                <Button
                  type="button"
                  onClick={(e) => {
                    e.preventDefault();
                    const newTuples = [
                      ...tuples,
                      generateDefaultValues(field.typ.items || []),
                    ];
                    handleChange(finalRootKey, newTuples);
                  }}
                >
                  Add Tuple
                </Button>
              </>
            );
          }}
        />
      );
    default:
      return <Typography>Some Data types were not configued.</Typography>;
  }
};

const DynamicForm: React.FC<{
  config: Parameter[];
  onSubmit: (data: FormData) => void;
}> = ({ config, onSubmit }) => {
  const defaultValues = useMemo(() => {
    return generateDefaultValues(config);
  }, [config]);

  const {
    control,
    handleSubmit,
    setValue,
    formState: { errors },
  } = useForm<FormData>({
    defaultValues: defaultValues,
  });

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const handleChange = (key: string, value: any) => {
    setValue(key, value, { shouldDirty: true });
  };

  return (
    <form onSubmit={handleSubmit(onSubmit)}>
      <Stack>
        {config.map((field, index) =>
          generateField(field, index, "", control, handleChange, errors)
        )}
      </Stack>
      <Button
        type="submit"
        variant="contained"
        color="primary"
        className="mt-2"
      >

        Submit
      </Button>
    </form>
  );
};

export default DynamicForm;
