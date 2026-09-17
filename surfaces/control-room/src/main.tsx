import React from "react";
import { createRoot } from "react-dom/client";

import { App } from "./App";
import "./styles/legacy.css";

const root = document.getElementById("root");
if (root === null) {
  throw new Error("Control Room root element is missing");
}

createRoot(root).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
