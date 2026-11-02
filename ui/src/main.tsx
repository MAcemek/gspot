import React from "react";
import ReactDOM from "react-dom/client";
import "./styles/app.css";
import { Panel } from "./windows/Panel";
import { GButton } from "./windows/GButton";
import { Underlines } from "./windows/Underlines";
import { Tab } from "./windows/Tab";
import { Card } from "./windows/Card";

import { getCurrentWindow } from "@tauri-apps/api/window";

let win = "panel";
try { win = getCurrentWindow().label; } catch { win = new URLSearchParams(window.location.search).get("win") ?? "panel"; }
const Root = win === "gbutton" ? GButton : win === "underlines" ? Underlines : win === "tab" ? Tab : win === "card" ? Card : Panel;
document.documentElement.dataset.win = win;
if (win !== "panel") document.body.style.background = "transparent";

import { invoke } from "@tauri-apps/api/core";
window.addEventListener("error", (e) => { invoke("ui_log", { message: `${win}: ${e.message} @ ${e.filename}:${e.lineno}` }).catch(() => {}); });
window.addEventListener("unhandledrejection", (e) => { invoke("ui_log", { message: `${win}: unhandled ${String((e as PromiseRejectionEvent).reason)}` }).catch(() => {}); });
invoke("ui_ready", { label: win }).catch(() => {});

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <Root />
  </React.StrictMode>,
);
