import React from "react";
import ReactDOM from "react-dom/client";
import { App } from "@/app/App";
import { PRODUCT_NAME } from "@/ipc/bindings";
import { tauriTransport, type Transport } from "@/ipc/transport";
import "@/styles/tokens.css";

/**
 * The Tauri transport, or the explicit mock under `npm run dev:mock`
 * (`vite --mode mock`). The mock is imported dynamically behind a
 * build-time mode check so production bundles never contain it.
 */
async function selectTransport(): Promise<Transport> {
  if (import.meta.env.MODE === "mock") {
    const { createMockTransport } = await import("@/test/mockTransport");
    return createMockTransport({ appInfo: { version: "0.0.0-mock", platform: "windows" } });
  }
  return tauriTransport;
}

const root = document.getElementById("root");

if (!root) {
  throw new Error("Root container not found");
}

document.title = PRODUCT_NAME;

void selectTransport().then((transport) => {
  ReactDOM.createRoot(root).render(
    <React.StrictMode>
      <App transport={transport} />
    </React.StrictMode>,
  );
});
