import * as React from "react";
import { Slot } from "@radix-ui/react-slot";
import { cva, type VariantProps } from "class-variance-authority";

import { cn } from "../../lib/utils";

const buttonVariants = cva(
  "inline-flex items-center justify-center gap-2 -nowrap text-sm font-medium ring-offset-background transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 disabled:pointer-events-none disabled:opacity-50 [&_svg]:pointer-events-none [&_svg]:size-4 [&_svg]:shrink-0",
  {
    variants: {
      variant: {
        default: "bg-button_bg border border-button_border",
        primary:"bg-button_bg border border-button_border dark:hover:bg-[#2C3B78] hover:bg-[#A0AAE9] dark:text-[#dddffe] text-[#1f2d5c]",
        dropdown: "border text-secondary-foreground hover:bg-secondary/80", 
        error: "bg-[#FFEFEF] dark:bg-[#3b191d] text-[#8e4e5a] dark:text-[#ffd1d9] border border-[#f9c6c6] dark:border-[#691D25] dark:hover:bg-[#782A2B] hover:bg-[#F29EA6] ",
        success:"border text-xs dark:text-[#c2f0c2] text-[#213c25] rounded-md dark:bg-[#18271d] #b7dfba dark:border-[#24452d] bg-[#ebf9eb] hover:bg-[#a2cea8] dark:hover:bg-[#2e5d36]",
        destructive:
          "bg-destructive text-destructive-foreground hover:bg-destructive/90",
        outline:
          "border border-input bg-background hover:bg-accent hover:text-accent-foreground",
        secondary:
          "bg-secondary text-secondary-foreground hover:bg-secondary/80",
        ghost: "hover:bg-accent hover:text-accent-foreground",
        link: "text-primary underline-offset-4 hover:underline",
        custom:
          "inline-flex items-center justify-center rounded-md text-sm font-medium px-4 py-2 h-10",
        inverted:""
      },
      size: {
        default: "h-10 px-4 py-2",
        sm: "h-9 rounded-md px-3",
        xs: "h-7 rounded-md px-2",
        md: "h-10 rounded-md px-4",
        lg: "h-11 rounded-md px-8",
        icon: "h-10 w-10",
        icon_sm: "h-5 rounded-sm px-2",
        info: "p-2 rounded-sm",
      },
    },
    defaultVariants: {
      variant: "default",
      size: "default",
    },
  }
);

export interface ButtonProps
  extends React.ButtonHTMLAttributes<HTMLButtonElement>,
    VariantProps<typeof buttonVariants> {
  asChild?: boolean;
  startIcon?: React.ReactNode;
  endIcon?: React.ReactNode;
  sx?: React.CSSProperties;
}

const Button = React.forwardRef<HTMLButtonElement, ButtonProps>(
  (
    {
      className,
      variant,
      size,
      asChild = false,
      startIcon,
      endIcon,
      ...props
    },
    ref
  ) => {
    const Comp = asChild ? Slot : "button";
    return (
      <Comp
        className={cn(buttonVariants({ variant, size, className }))}
        ref={ref}
        {...props}
      >
        {startIcon && <span className="mr-1">{startIcon}</span>}
        {props.children}
        {endIcon && <span className="ml-1">{endIcon}</span>}
      </Comp>
    );
  }
);
Button.displayName = "Button";

export { Button as Button2, buttonVariants };
