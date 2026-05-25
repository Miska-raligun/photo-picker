import { Languages } from "lucide-react";
import { Button } from "@/components/ui/button";
import { useI18n } from "@/lib/i18n";

export function LanguageToggle() {
  const { lang, setLang } = useI18n();
  return (
    <Button
      variant="ghost"
      size="sm"
      onClick={() => setLang(lang === "zh" ? "en" : "zh")}
      className="gap-1.5 text-muted-foreground hover:text-foreground"
      aria-label="toggle language"
    >
      <Languages className="h-4 w-4" />
      <span className="text-xs font-medium">{lang === "zh" ? "中文" : "EN"}</span>
    </Button>
  );
}
