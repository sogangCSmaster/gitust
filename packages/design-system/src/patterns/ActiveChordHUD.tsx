import { Show, type Component } from "solid-js";
import { Card, CardContent } from "../components/ui/card";
import { KeyHint } from "../components/workspace/presentation";
import { shortcutManager } from "../keyboard/shortcut-manager";
export const ActiveChordHUD: Component = () => (
  <Show when={shortcutManager.activeChord()}>
    {(prefix) => (
      <Card class="fixed bottom-5 left-1/2 z-40 -translate-x-1/2 shadow-lg">
        <CardContent
          class="flex items-center gap-3 px-4 py-3 text-xs"
          role="status"
        >
          <KeyHint keys={prefix()} />
          <span class="whitespace-nowrap text-muted-foreground">
            Waiting for the next key…
          </span>
        </CardContent>
      </Card>
    )}
  </Show>
);
