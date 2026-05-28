import { useEffect, useState } from "react";
import { Moon, Sun } from "lucide-react";
import { useTheme } from "next-themes";
import { AnimatePresence, motion } from "motion/react";
import { Button } from "@/components/ui/button";
import { useM } from "@/lib/i18n";

/// Light/dark toggle. Renders an invisible placeholder until mounted to avoid
/// the SSR/CSR-style class mismatch warning from next-themes; in our pure-CSR
/// app this means the first render still matches the server-less HTML.
export function ThemeToggle() {
  const m = useM();
  const { resolvedTheme, setTheme } = useTheme();
  const [mounted, setMounted] = useState(false);
  useEffect(() => setMounted(true), []);

  const isDark = mounted && resolvedTheme === "dark";
  const toggle = () => setTheme(isDark ? "light" : "dark");

  return (
    <Button
      variant="ghost"
      size="sm"
      onClick={toggle}
      className="text-muted-foreground hover:text-foreground relative h-8 w-8 p-0"
      aria-label={isDark ? m.common.themeLight : m.common.themeDark}
    >
      <AnimatePresence mode="wait" initial={false}>
        <motion.span
          key={isDark ? "moon" : "sun"}
          initial={{ opacity: 0, rotate: -45, scale: 0.6 }}
          animate={{ opacity: 1, rotate: 0, scale: 1 }}
          exit={{ opacity: 0, rotate: 45, scale: 0.6 }}
          transition={{ duration: 0.18, ease: "easeOut" }}
          className="inline-flex"
        >
          {isDark ? <Moon className="h-4 w-4" /> : <Sun className="h-4 w-4" />}
        </motion.span>
      </AnimatePresence>
    </Button>
  );
}
