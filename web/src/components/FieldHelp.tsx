import { Label } from "@/components/ui/label";
import { cn } from "@/lib/utils";

interface Props {
  label: React.ReactNode;
  htmlFor?: string;
  desc?: React.ReactNode;
  children: React.ReactNode;
  className?: string;
}

/**
 * Form-field wrapper with a label, optional plain-language description below,
 * and the input slot. Standard pattern across the scan form.
 */
export function FieldHelp({ label, htmlFor, desc, children, className }: Props) {
  return (
    <div className={cn("grid gap-1.5", className)}>
      <Label htmlFor={htmlFor} className="text-sm font-medium">
        {label}
      </Label>
      {desc && (
        <p className="text-[0.78rem] text-muted-foreground leading-snug">
          {desc}
        </p>
      )}
      <div className="mt-0.5">{children}</div>
    </div>
  );
}
