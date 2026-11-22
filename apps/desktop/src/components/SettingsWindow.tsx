import { useState } from "react";
import { MicrophoneSelector } from "./MicrophoneSelector";

type SettingsTab = "onboarding" | "general" | "vocabulary" | "history";

export function SettingsWindow() {
  const [activeTab, setActiveTab] = useState<SettingsTab>("onboarding");

  const tabs: { id: SettingsTab; label: string }[] = [
    { id: "onboarding", label: "Provider Credential & Permissions" },
    { id: "general", label: "General" },
    { id: "vocabulary", label: "Custom Vocabulary" },
    { id: "history", label: "History" },
  ];

  return (
    <div className="min-h-screen bg-slate-950 text-slate-100">
      <header className="border-b border-slate-800 px-6 py-4">
        <h1 className="text-lg font-semibold">EchoFlow Settings & History</h1>
        <p className="text-xs text-slate-400">
          Local-first BYOK desktop voice dictation powered by
          gemini-3.5-transcribe-live
        </p>
      </header>

      <nav className="flex gap-2 border-b border-slate-800 px-6 py-2">
        {tabs.map((tab) => (
          <button
            key={tab.id}
            type="button"
            onClick={() => setActiveTab(tab.id)}
            className={`rounded-md px-3 py-1.5 text-sm font-medium transition ${
              activeTab === tab.id
                ? "bg-slate-800 text-white"
                : "text-slate-400 hover:text-slate-200"
            }`}
          >
            {tab.label}
          </button>
        ))}
      </nav>

      <main className="p-6 text-sm text-slate-300">
        {activeTab === "onboarding" && (
          <section>
            <h2 className="mb-2 text-base font-medium text-white">
              Provider Credential & OS Permissions
            </h2>
            <p className="text-slate-400">
              Configure your Gemini API key in the OS native credential store
              and verify platform capabilities.
            </p>
          </section>
        )}
        {activeTab === "general" && (
          <section className="space-y-4">
            <div>
              <h2 className="mb-2 text-base font-medium text-white">
                Push-to-Talk Hotkey &amp; Microphone
              </h2>
              <p className="text-slate-400">
                Customize your Push-to-Talk Hotkey, microphone device, and
                Clipboard Preservation settings.
              </p>
            </div>
            <MicrophoneSelector />
          </section>
        )}
        {activeTab === "vocabulary" && (
          <section>
            <h2 className="mb-2 text-base font-medium text-white">
              Custom Vocabulary (0 / 1,000)
            </h2>
            <p className="text-slate-400">
              Manage domain jargon and proper nouns for recognition biasing.
            </p>
          </section>
        )}
        {activeTab === "history" && (
          <section>
            <h2 className="mb-2 text-base font-medium text-white">
              History Entry Archive
            </h2>
            <p className="text-slate-400">
              Text-only local records of completed Dictation Sessions.
            </p>
          </section>
        )}
      </main>
    </div>
  );
}
