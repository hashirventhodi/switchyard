import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { detachedCore, insideTauri, tauriCore, type CoreConnection } from "./core";
import "./tokens.css";
import "./frame.css";
import "./session.css";

async function pickFolder(): Promise<string | null> {
  if (!insideTauri()) return window.prompt("Repository path") || null;
  const { open } = await import("@tauri-apps/plugin-dialog");
  const picked = await open({ directory: true, multiple: false, title: "Open a repository" });
  return typeof picked === "string" ? picked : null;
}

/** Install and update links open in the user's browser, never inside the app window. */
async function openLink(url: string) {
  if (!insideTauri()) return void window.open(url, "_blank", "noopener");
  const { invoke } = await import("@tauri-apps/api/core");
  await invoke("open_link", { url });
}

const connect: Promise<CoreConnection> = insideTauri() ? tauriCore() : Promise.resolve(detachedCore());

connect.then((core) => {
  createRoot(document.getElementById("root")!).render(
    <StrictMode>
      <App core={core} pickFolder={pickFolder} openLink={openLink} />
    </StrictMode>,
  );
});
