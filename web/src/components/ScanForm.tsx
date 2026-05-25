import { useState } from "react";
import { ArrowRight, FolderOpen, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Badge } from "@/components/ui/badge";
import { BrowseDialog, type BrowseResult } from "./BrowseDialog";
import { FieldHelp } from "./FieldHelp";
import { TaskConfigDialog } from "./TaskConfigDialog";
import { useM } from "@/lib/i18n";
import { toast } from "sonner";

interface Props {
  onScanStarted: (runId: string, summary: string, output: string) => void;
}

export function ScanForm({ onScanStarted }: Props) {
  const m = useM();
  const [root, setRoot] = useState("");
  const [output, setOutput] = useState("");
  const [explicitFiles, setExplicitFiles] = useState<{
    files: string[];
    sourceDir: string;
  } | null>(null);

  const [browseOpen, setBrowseOpen] = useState(false);
  const [browseMode, setBrowseMode] = useState<"source" | "output">("source");
  const [configOpen, setConfigOpen] = useState(false);

  function openBrowser(mode: "source" | "output") {
    setBrowseMode(mode);
    setBrowseOpen(true);
  }

  function handleBrowseConfirm(result: BrowseResult) {
    if (browseMode === "output") {
      if (result.kind === "folder") setOutput(result.path);
    } else if (result.kind === "files") {
      setExplicitFiles({ files: result.files, sourceDir: result.sourceDir });
      setRoot("");
    } else {
      setRoot(result.path);
      setExplicitFiles(null);
    }
  }

  function createTask() {
    if (!root && !explicitFiles) {
      toast.error(m.errors.pickSource);
      return;
    }
    setConfigOpen(true);
  }

  const inPlace = !output.trim();
  const effectiveSource = explicitFiles ? explicitFiles.sourceDir : root;

  return (
    <>
      <Card>
        <CardHeader className="pb-4">
          <CardTitle className="text-sm uppercase tracking-wider text-muted-foreground font-semibold">
            {m.scanForm.title}
          </CardTitle>
        </CardHeader>
        <CardContent>
          <div className="grid gap-5">
            <FieldHelp
              label={m.scanForm.source}
              htmlFor="root"
              desc={m.scanForm.sourceDesc}
            >
              <div className="flex gap-2">
                <Input
                  id="root"
                  value={root}
                  onChange={(e) => {
                    setRoot(e.target.value);
                    setExplicitFiles(null);
                  }}
                  placeholder={m.scanForm.sourcePlaceholder}
                  className="font-mono text-sm"
                />
                <Button
                  type="button"
                  variant="outline"
                  onClick={() => openBrowser("source")}
                >
                  <FolderOpen className="h-4 w-4" />
                  {m.scanForm.browse}
                </Button>
              </div>
              {explicitFiles && (
                <Badge
                  variant="secondary"
                  className="bg-accent text-accent-foreground gap-1.5 self-start mt-2"
                >
                  {explicitFiles.files.length} photos from{" "}
                  {explicitFiles.sourceDir}
                  <button
                    type="button"
                    onClick={() => setExplicitFiles(null)}
                    className="ml-1 hover:opacity-70"
                    aria-label="clear selection"
                  >
                    <X className="h-3 w-3" />
                  </button>
                </Badge>
              )}
            </FieldHelp>

            <FieldHelp
              label={m.scanForm.output}
              htmlFor="output"
              desc={m.scanForm.outputDesc}
            >
              <div className="flex gap-2">
                <Input
                  id="output"
                  value={output}
                  onChange={(e) => setOutput(e.target.value)}
                  placeholder={m.scanForm.outputPlaceholder}
                  className="font-mono text-sm"
                />
                <Button
                  type="button"
                  variant="outline"
                  onClick={() => openBrowser("output")}
                >
                  <FolderOpen className="h-4 w-4" />
                  {m.scanForm.browse}
                </Button>
              </div>
            </FieldHelp>

            <div className="flex justify-end pt-2">
              <Button onClick={createTask} size="lg">
                {m.scanForm.createTask}
                <ArrowRight className="h-4 w-4" />
              </Button>
            </div>
          </div>
        </CardContent>
      </Card>

      <BrowseDialog
        open={browseOpen}
        onOpenChange={setBrowseOpen}
        mode={browseMode}
        initialPath={browseMode === "source" ? root : output}
        onConfirm={handleBrowseConfirm}
      />

      <TaskConfigDialog
        open={configOpen}
        onOpenChange={setConfigOpen}
        source={effectiveSource}
        files={explicitFiles?.files ?? null}
        output={output}
        inPlace={inPlace}
        onStarted={onScanStarted}
      />
    </>
  );
}
