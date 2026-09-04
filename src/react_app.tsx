import React, { useEffect } from "react";
import { createRoot } from "react-dom/client";
import { ConfigProvider } from "@arco-design/web-react";
import "@arco-design/web-react/dist/css/arco.css";
import "./react_ui.css";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { AppProvider, useAppState } from "./ui/context";
import { ConfigPage, OverlayApp } from "./ui/components";

function ThemeSync({ overlay }: { overlay: boolean }) {
  const { snapshot } = useAppState();
  useEffect(() => {
    const theme = snapshot.config.overlay_theme || "follow";
    document.documentElement.dataset.theme = theme;
    document.documentElement.classList.toggle("arco-theme-dark", theme === "night");
    document.body.classList.toggle("arco-theme-dark", theme === "night");
    document.documentElement.classList.toggle("overlay-window", overlay);
    document.body.classList.toggle("overlay-window", overlay);
    return () => {
      document.documentElement.classList.remove("overlay-window");
      document.body.classList.remove("overlay-window");
    };
  }, [snapshot.config.overlay_theme, overlay]);
  return null;
}

function App() {
  const overlay = getCurrentWindow().label === "overlay";
  return <ConfigProvider size="default"><AppProvider><ThemeSync overlay={overlay} />{overlay ? <OverlayApp /> : <ConfigPage />}</AppProvider></ConfigProvider>;
}

const root = document.getElementById("app");
if (root) createRoot(root).render(<App />);
