import type { LucideIcon } from "lucide-react";
import { Check } from "lucide-react";
import { motion, useReducedMotion } from "motion/react";
import { cn } from "@/lib/utils";

interface Props {
  selected: boolean;
  onSelect: () => void;
  icon: LucideIcon;
  title: string;
  hint: string;
  accent?: "primary" | "neutral";
}

/// Clickable preset tile for task creation. Selection state shows a soft ring
/// + check; hover lifts slightly. Reduced-motion disables the lift.
export function PresetCard({
  selected,
  onSelect,
  icon: Icon,
  title,
  hint,
  accent = "neutral",
}: Props) {
  const reduce = useReducedMotion();
  return (
    <motion.button
      type="button"
      onClick={onSelect}
      whileHover={reduce ? undefined : { y: -2 }}
      whileTap={reduce ? undefined : { scale: 0.99 }}
      transition={{ duration: 0.14, ease: "easeOut" }}
      aria-pressed={selected}
      className={cn(
        "relative text-left rounded-xl border bg-card px-4 py-3.5",
        "transition-shadow transition-colors",
        "hover:shadow-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/50",
        selected
          ? "border-primary/50 ring-2 ring-primary/25 bg-accent/40"
          : "border-border hover:border-foreground/15"
      )}
    >
      <div className="flex items-start gap-3">
        <div
          className={cn(
            "shrink-0 grid place-items-center h-8 w-8 rounded-lg",
            accent === "primary" || selected
              ? "bg-primary/10 text-primary"
              : "bg-muted text-muted-foreground"
          )}
        >
          <Icon className="h-4 w-4" />
        </div>
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-1.5">
            <span className="text-sm font-semibold leading-none">{title}</span>
            {selected && (
              <Check className="h-3.5 w-3.5 text-primary" aria-hidden="true" />
            )}
          </div>
          <p className="mt-1 text-[0.78rem] leading-snug text-muted-foreground">
            {hint}
          </p>
        </div>
      </div>
    </motion.button>
  );
}
