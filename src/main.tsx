import ReactDOM from "react-dom/client";
import App from "./App";
import "./styles.css";

// No StrictMode: App owns a Monaco instance and restores the session once on
// mount, which double-invoked effects would duplicate.
ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(<App />);
