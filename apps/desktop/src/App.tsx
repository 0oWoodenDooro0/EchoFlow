import { OverlayPill } from "./components/OverlayPill";
import { SettingsWindow } from "./components/SettingsWindow";

export function App() {
  const params = new URLSearchParams(window.location.search);
  const view = params.get("view");

  if (view === "overlay") {
    return <OverlayPill />;
  }

  return <SettingsWindow />;
}

export default App;
