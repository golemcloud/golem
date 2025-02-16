import { AlertCircle } from "lucide-react";

export const FormInput = ({
    label,
    name,
    error,
    touched,
    ...props
}: {
    label: string;
    name: string;
    error?: string;
    touched?: boolean;
} & React.InputHTMLAttributes<HTMLInputElement>) => (
    <div>
        <label className="block text-sm font-medium mb-1.5 text-foreground/90">
            {label}
        </label>
        <div className="relative">
            <input
                {...props}
                name={name}
                className={`w-full px-4 py-2.5 bg-card/50 rounded-lg border
            ${touched && error ? "border-destructive" : "border-border"}
            focus:border-primary focus:ring-1 focus:ring-primary outline-none
            transition-all duration-200`}
            />
            {touched && error && (
                <div className="mt-1 flex items-center gap-1 text-destructive text-sm">
                    <AlertCircle size={14} className="flex-shrink-0" />
                    <span>{error}</span>
                </div>
            )}
        </div>
    </div>
);

export default FormInput;