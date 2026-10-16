import { ErrorBoundary, Suspense, type ParentComponent } from "solid-js";
import { Button } from "../ui/button";

/** Keep a failed module download visible and recoverable without hiding the workspace. */
export const DeferredView: ParentComponent<{
  label: string;
  onCancel?: () => void;
}> = (props) => (
  <ErrorBoundary
    fallback={() => (
      <div role="alert" class="space-y-3 rounded-lg border bg-card p-4 text-sm">
        <p>
          Could not load {props.label}. Reload to get the current application
          files.
        </p>
        <div class="flex gap-2">
          <Button onClick={() => window.location.reload()}>
            Reload application
          </Button>
          {props.onCancel && (
            <Button variant="outline" onClick={props.onCancel}>
              Close
            </Button>
          )}
        </div>
      </div>
    )}
  >
    <Suspense
      fallback={
        <p role="status" class="p-4 text-sm text-muted-foreground">
          Opening {props.label}…
        </p>
      }
    >
      {props.children}
    </Suspense>
  </ErrorBoundary>
);
