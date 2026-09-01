import React from "react";
import { createRoot } from "react-dom/client";

import { App } from "./App";
import "./styles/base.css";
import "./styles/workspace.css";
import "./styles/history.css";
import "./styles/lock.css";
import "./styles/responsive.css";

const root = document.getElementById("root");
if (root === null) {
  throw new Error("Control Room root element is missing");
}

createRoot(root).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
