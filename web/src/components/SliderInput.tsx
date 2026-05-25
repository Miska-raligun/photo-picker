import { Slider } from "@/components/ui/slider";
import { Input } from "@/components/ui/input";
import { cn } from "@/lib/utils";

interface Props {
  value: number;
  onChange: (v: number) => void;
  min?: number;
  max?: number;
  step?: number;
  className?: string;
  inputClassName?: string;
  /** Optional formatting for the number display. */
  format?: (v: number) => string;
}

export function SliderInput({
  value,
  onChange,
  min = 0,
  max = 1,
  step = 0.01,
  className,
  inputClassName,
}: Props) {
  return (
    <div className={cn("flex items-center gap-3", className)}>
      <Slider
        value={[value]}
        min={min}
        max={max}
        step={step}
        onValueChange={(v) => onChange(v[0])}
        className="flex-1"
      />
      <Input
        type="number"
        min={min}
        max={max}
        step={step}
        value={value}
        onChange={(e) => {
          const v = parseFloat(e.target.value);
          if (!Number.isNaN(v)) onChange(v);
        }}
        className={cn("w-20 h-8 text-sm font-mono text-center", inputClassName)}
      />
    </div>
  );
}
