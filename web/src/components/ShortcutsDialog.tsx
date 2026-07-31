import { useEffect } from "react";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { useM } from "@/lib/i18n";

/// Global keyboard reference. The app accumulated a dozen shortcuts across
/// the grid and lightbox with no single place to see them; `?` opens this.
export function ShortcutsDialog({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (v: boolean) => void;
}) {
  const m = useM();
  const groups: { title: string; items: [string, string][] }[] = [
    {
      title: m.shortcuts.groupGrid,
      items: [
        ["Click", m.shortcuts.gridClick],
        ["Shift + Click", m.shortcuts.gridShift],
        ["Ctrl/⌘ + Click", m.shortcuts.gridCtrl],
        ["Esc", m.shortcuts.gridEsc],
        ["← / →", m.shortcuts.gridArrows],
      ],
    },
    {
      title: m.shortcuts.groupLightbox,
      items: [
        ["← / →", m.shortcuts.lbArrows],
        ["Home / End", m.shortcuts.lbHomeEnd],
        ["PgUp / PgDn", m.shortcuts.lbPage],
        ["Space", m.shortcuts.lbSpace],
        ["I", m.shortcuts.lbInfo],
        ["+ / − / 0", m.shortcuts.lbZoom],
        [m.shortcuts.lbWheelKey, m.shortcuts.lbWheel],
        ["Esc", m.shortcuts.lbEsc],
      ],
    },
    {
      title: m.shortcuts.groupGlobal,
      items: [["?", m.shortcuts.globalHelp]],
    },
  ];

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-lg">
        <DialogHeader>
          <DialogTitle>{m.shortcuts.title}</DialogTitle>
        </DialogHeader>
        <div className="space-y-4 py-1 max-h-[70vh] overflow-y-auto">
          {groups.map((g) => (
            <div key={g.title} className="space-y-1.5">
              <h3 className="text-xs font-semibold uppercase tracking-wider text-muted-foreground">
                {g.title}
              </h3>
              <dl className="space-y-1">
                {g.items.map(([key, desc]) => (
                  <div key={key} className="flex items-baseline gap-3 text-sm">
                    <dt className="shrink-0">
                      <kbd className="font-mono text-[0.7rem] bg-muted border rounded px-1.5 py-0.5">
                        {key}
                      </kbd>
                    </dt>
                    <dd className="text-muted-foreground">{desc}</dd>
                  </div>
                ))}
              </dl>
            </div>
          ))}
        </div>
      </DialogContent>
    </Dialog>
  );
}

/// Bind `?` to open the shortcuts dialog. Ignores the key while the user is
/// typing so it can't hijack a note or a path field.
export function useShortcutsHotkey(onOpen: () => void) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "?") return;
      const tag = (e.target as HTMLElement | null)?.tagName;
      if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") return;
      e.preventDefault();
      onOpen();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onOpen]);
}
