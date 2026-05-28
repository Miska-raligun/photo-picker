import type { CSSProperties, ReactNode } from "react";
import { motion, useReducedMotion, type Transition } from "motion/react";

/// Restrained entrance animation: subtle fade + 6px upward slide. No-op under
/// `prefers-reduced-motion` so the OS preference is respected end-to-end.
export function FadeUp({
  children,
  delay = 0,
  className,
  style,
}: {
  children: ReactNode;
  delay?: number;
  className?: string;
  style?: CSSProperties;
}) {
  const reduce = useReducedMotion();
  const transition: Transition = reduce
    ? { duration: 0 }
    : { duration: 0.24, delay, ease: [0.22, 0.6, 0.36, 1] };
  return (
    <motion.div
      initial={reduce ? false : { opacity: 0, y: 6 }}
      animate={{ opacity: 1, y: 0 }}
      transition={transition}
      className={className}
      style={style}
    >
      {children}
    </motion.div>
  );
}
