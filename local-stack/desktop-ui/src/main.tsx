import React from "react";
import ReactDOM from "react-dom/client";
import { Dashboard } from "./Dashboard";
import { I18nProvider } from "./i18n";
import "./index.css";

ReactDOM.createRoot(document.getElementById("app")!).render(
  <React.StrictMode>
    <I18nProvider>
      <Dashboard />
    </I18nProvider>
  </React.StrictMode>,
);
