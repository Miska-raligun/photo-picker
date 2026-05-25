import { Component, type ErrorInfo, type ReactNode } from "react";
import { AlertCircle, RotateCcw } from "lucide-react";
import { Button } from "@/components/ui/button";

interface Props {
  children: ReactNode;
  /// Optional reset key — change it to force-clear the error and remount.
  resetKey?: string | number;
}

interface State {
  error: Error | null;
}

/**
 * Catches render errors so a single bad run record / bad VLM response
 * doesn't white-screen the whole app. Logs to console; offers a "reset"
 * that clears the error state.
 */
export class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error("ErrorBoundary caught render error:", error, info);
  }

  componentDidUpdate(prevProps: Props) {
    if (prevProps.resetKey !== this.props.resetKey && this.state.error) {
      this.setState({ error: null });
    }
  }

  reset = () => this.setState({ error: null });

  render() {
    if (this.state.error) {
      return (
        <div className="rounded-xl border border-destructive/30 bg-destructive/5 p-5 my-3 space-y-3">
          <div className="flex items-center gap-2 text-destructive font-semibold">
            <AlertCircle className="h-4 w-4" />
            Something went wrong rendering this section
          </div>
          <pre className="text-xs font-mono whitespace-pre-wrap text-destructive/80 max-h-40 overflow-auto">
            {this.state.error.message}
          </pre>
          <div className="flex justify-end">
            <Button size="sm" variant="outline" onClick={this.reset}>
              <RotateCcw className="h-3 w-3" />
              Reset
            </Button>
          </div>
        </div>
      );
    }
    return this.props.children;
  }
}
