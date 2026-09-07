import React, { Component, type ErrorInfo, type ReactNode } from "react";
import ReactDOM from "react-dom/client";
import { App } from "./App";
import "./index.css";

interface Props {
  children: ReactNode;
}

interface State {
  hasError: boolean;
  error: Error | null;
  errorInfo: ErrorInfo | null;
}

class RootErrorBoundary extends Component<Props, State> {
  public state: State = {
    hasError: false,
    error: null,
    errorInfo: null,
  };

  public static getDerivedStateFromError(error: Error): State {
    return { hasError: true, error, errorInfo: null };
  }

  public componentDidCatch(error: Error, errorInfo: ErrorInfo) {
    console.error("[RootErrorBoundary] Caught uncaught error:", error, errorInfo);
    this.setState({ error, errorInfo });
  }

  public render() {
    if (this.state.hasError) {
      return (
        <div style={{
          position: "fixed",
          inset: 0,
          background: "#fff1f2",
          color: "#991b1b",
          padding: "32px",
          fontFamily: "monospace",
          overflow: "auto",
          zIndex: 999999,
        }}>
          <h1 style={{ fontSize: "22px", fontWeight: "bold", color: "#e11d48", marginBottom: "12px" }}>
            Application Error Caught
          </h1>
          <p style={{ fontSize: "16px", marginBottom: "12px" }}>
            {this.state.error?.message || "An unexpected error occurred during rendering."}
          </p>
          <pre style={{
            background: "#ffffff",
            padding: "16px",
            border: "1px solid #fda4af",
            borderRadius: "6px",
            overflow: "auto",
            whiteSpace: "pre-wrap",
            fontSize: "13px",
          }}>
            {this.state.error?.stack}
            {"\n\nComponent Stack:\n"}
            {this.state.errorInfo?.componentStack}
          </pre>
        </div>
      );
    }

    return this.props.children;
  }
}

const rootElement = document.getElementById("root");
if (rootElement) {
  try {
    const root = ReactDOM.createRoot(rootElement);
    root.render(
      <React.StrictMode>
        <RootErrorBoundary>
          <App />
        </RootErrorBoundary>
      </React.StrictMode>
    );
  } catch (err) {
    console.error("[main.tsx] Failed to createRoot or render:", err);
  }
} else {
  console.error("[main.tsx] Root DOM node '#root' not found!");
}
