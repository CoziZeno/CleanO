import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
import "./app/theme.css";
import "./app/ui.css";

const container = document.getElementById("root");
if (!container) throw new Error("#root is missing from index.html");

// Suppress the webview's own context menu ("Reload", "Inspect", "Save image
// as…"). It is a browser menu in a desktop app and immediately breaks the
// illusion; the app supplies its own. Text selections keep theirs, so copying
// an error message still works.
document.addEventListener("contextmenu", (event) => {
  const target = event.target as HTMLElement | null;
  const editable =
    target?.tagName === "INPUT" ||
    target?.tagName === "TEXTAREA" ||
    target?.isContentEditable === true;
  const hasSelection = (window.getSelection()?.toString().length ?? 0) > 0;
  if (!editable && !hasSelection) event.preventDefault();
});

// Dragging a file onto the window should not navigate it away.
document.addEventListener("dragover", (event) => event.preventDefault());
document.addEventListener("drop", (event) => event.preventDefault());

createRoot(container).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
