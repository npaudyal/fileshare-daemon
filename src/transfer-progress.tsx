import React from "react";
import ReactDOM from "react-dom/client";
import TransferProgressWindow from "./components/transfer/TransferProgressWindow";
import { ThemeProvider } from "./context/ThemeContext";
import "./index.css";

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <ThemeProvider>
      <TransferProgressWindow />
    </ThemeProvider>
  </React.StrictMode>
);