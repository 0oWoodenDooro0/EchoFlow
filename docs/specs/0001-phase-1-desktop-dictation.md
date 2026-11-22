# Phase 1: Cross-Platform Desktop Voice Dictation (BYOK)

## Problem Statement

Users who want fast, polished voice-to-text dictation across every desktop application (chat apps, browsers, IDEs, terminals, email clients) face three recurring frustrations:

1. **Vendor lock-in and subscriptions**: Existing polished dictation tools require monthly subscriptions and route user audio through proprietary third-party servers, even when users already have access to their own Google Gemini API key.
2. **Clumsy raw transcription**: Traditional speech-to-text outputs literal transcripts full of filler words, false starts, missing punctuation, and mangled domain jargon, forcing heavy manual editing.
3. **Broken cross-app insertion**: Live character-by-character typing frequently corrupts text in terminals, browsers, or active CJK input methods (IMEs), especially when a streaming model revises earlier words mid-sentence.

## Solution

Build **EchoFlow**, a zero-backend, local-first Bring-Your-Own-Key (BYOK) desktop dictation application for macOS, Windows, and Linux powered exclusively by `gemini-3.5-transcribe-live` in native `SMART` mode.

Users hold a customizable **Push-to-Talk Hotkey** in any application to start a **Dictation Session**. While speaking, a non-focus-stealing **Overlay Pill** displays a live audio waveform and real-time **Draft Transcript** as the model cleans filler words, applies punctuation, and resolves mid-sentence self-corrections. When the user releases the hotkey, the finalized **Final Transcript** is inserted cleanly into the active cursor position via **Text Insertion** with **Clipboard Preservation**, and saved locally as a text-only **History Entry** (no audio persisted). Users manage their **Provider Credential** in the OS native credential store and boost recognition accuracy for names and jargon via a local **Custom Vocabulary** (up to 1,000 entries).

## User Stories

1. As a desktop user, I want to enter my Gemini API key during first-run setup and save it as a **Provider Credential** in my OS native credential store, so that my key is never written in plaintext config files.
2. As a desktop user, I want to click a "Test Connection" button after entering my **Provider Credential**, so that I immediately know whether my key can reach `gemini-3.5-transcribe-live` before attempting to dictate.
3. As a macOS user, I want a first-run onboarding checklist that detects and guides me through granting Microphone and Accessibility permissions, so that global hotkeys and **Text Insertion** work without mysterious silent failures.
4. As a Linux user, I want the app to detect whether I am running on X11 or Wayland and report which input/paste capabilities are available, so that I understand whether I have full **Text Insertion** or **Fallback Delivery**.
5. As a desktop user, I want to press and hold my **Push-to-Talk Hotkey** from any application to start a **Dictation Session**, so that I can dictate without switching windows.
6. As a desktop user, I want to customize my **Push-to-Talk Hotkey** in Settings, so that it never conflicts with my IDE, browser, or OS shortcuts.
7. As a desktop user, I want a non-focus-stealing **Overlay Pill** to appear while I hold the **Push-to-Talk Hotkey**, so that my cursor stays in the target application while I see live visual confirmation that audio is streaming.
8. As a desktop user, I want to see a live audio level waveform in the **Overlay Pill**, so that I immediately notice if my microphone is muted or picking up the wrong input device.
9. As a desktop user, I want to see the **Draft Transcript** streaming and self-correcting in real time inside the **Overlay Pill**, so that I can verify what the model heard before releasing the hotkey.
10. As a desktop user, I want `gemini-3.5-transcribe-live` `SMART` mode to automatically remove filler words, handle mid-sentence self-corrections, format numbers, and apply punctuation across 85+ languages including code-switching (e.g., Traditional Chinese + English), so that my dictated text is ready to send without manual cleanup.
11. As a desktop user, I want releasing the **Push-to-Talk Hotkey** to end the **Dictation Session** and deliver the **Final Transcript** into my active cursor position via **Text Insertion**, so that text appears cleanly in any app (Slack, Notion, browsers, IDEs, terminals).
12. As a desktop user, I want **Clipboard Preservation** enabled by default during **Text Insertion**, so that whatever I previously copied is automatically restored after dictation finishes.
13. As a desktop user, I want a setting to disable automatic clipboard restoration in **Clipboard Preservation**, so that I can keep the **Final Transcript** on my clipboard if I prefer pasting it multiple times.
14. As a desktop user, I want **Fallback Delivery** to automatically keep the **Final Transcript** in my clipboard and notify me via the **Overlay Pill** whenever **Text Insertion** cannot complete (e.g., no editable focus or restricted Wayland session), so that I never lose what I dictated.
15. As a desktop user, I want to switch my active microphone input device directly from the system tray menu or Settings, so that I can quickly change between headset, laptop mic, and external USB mic.
16. As a desktop user, I want to see the app's idle, listening, error, or missing-credential state reflected in the system tray icon/menu, so that I always know whether dictation is ready.
17. As a desktop user, I want to add, edit, search, and delete terms in my **Custom Vocabulary** (up to 1,000 entries), so that domain jargon, product names, and people's names are transcribed accurately via Gemini's native vocabulary biasing.
18. As a desktop user, I want clear feedback if I attempt to exceed the 1,000-entry **Custom Vocabulary** limit, so that I never silently drop vocabulary terms when starting a **Dictation Session**.
19. As a desktop user, I want every completed **Dictation Session** to be saved locally as a text-only **History Entry** with its **Final Transcript** and timestamp (and zero audio saved), so that I can retrieve past dictations while keeping disk usage and privacy exposure minimal.
20. As a desktop user, I want to search my local **History Entry** list and copy any entry with one click, so that I can recover text if I accidentally pasted it into the wrong window.
21. As a desktop user, I want old **History Entry** records to be automatically pruned after a configurable retention period (default 30 days) and to have a one-click "Clear All History" action, so that my local history stays tidy and private.
22. As a desktop user, I want to export and import my non-secret settings and **Custom Vocabulary** as a JSON file (explicitly excluding my **Provider Credential**), so that I can back up or transfer my setup across machines without needing a cloud account.
23. As a desktop user, I want a visible countdown warning in the **Overlay Pill** at 9:30 during a long **Dictation Session** and automatic finalization with **Text Insertion** at the 10:00 API limit, so that hitting the session cap never abruptly discards my speech.
24. As a desktop user, I want any partial transcript already received before an unexpected network disconnect or API error to be preserved as a **History Entry** and placed in my clipboard with a clear error message in the **Overlay Pill**, so that flaky Wi-Fi never makes spoken text vanish.
25. As a desktop user, I want closing the Settings window to release its WebView resources while keeping the native system tray and Rust core running in the background, so that the always-on app consumes minimal idle memory.
26. As a desktop user, I want to cancel an active **Dictation Session** (e.g., pressing Escape while holding the hotkey) without performing **Text Insertion** or creating a **History Entry**, so that I can abort accidental triggers cleanly.

## Implementation Decisions

### Architectural Shape (ADR-0001, ADR-0004)
- **Workspace separation**: Split the codebase into a platform-agnostic **Rust Core** library and a **Desktop Shell** built with Tauri v2 + React + TypeScript + Tailwind CSS (Vite). The Rust Core owns all domain logic, audio capture/resampling, WebSocket streaming, local SQLite persistence, and settings/export logic so Phase 2 native mobile shells (iOS Swift / Android Kotlin via UniFFI) can reuse it directly without rewriting core logic.
- **Zero-backend BYOK**: All network traffic goes directly from the user's device to Google's `gemini-3.5-transcribe-live` WebSocket endpoint using the user's **Provider Credential**. No backend server, telemetry, or account layer exists.
- **Low idle memory**: The system tray menu is implemented natively in Rust via Tauri. The Settings & History window destroys/releases its WebView when closed rather than hiding it in memory. The **Overlay Pill** is a minimal, always-on-top, non-focus-stealing window shown only during active sessions or transient notifications.

### Core Modules & Interfaces
- **DictationController (Core Coordinator)**: Single top-level orchestrator owning the **Dictation Session** state machine:
  - States: `Idle` → `Connecting` → `Listening` → `Finalizing` → `Delivering` → `Idle` (plus transient `Error` / `FallbackNotice` display states).
  - Enforces single active **Dictation Session** at a time (ignores duplicate hotkey presses while active).
  - Runs the 9:30 countdown warning timer and 10:00 hard auto-finalization timer (ADR-0002).
  - Handles abort (Escape) by closing the stream without **Text Insertion** or persisting a **History Entry**.
  - On mid-session network disconnect or API error, salvages the latest non-empty **Draft Transcript** into both the clipboard and a **History Entry** (marked as partial/interrupted) and emits an error event to the **Overlay Pill**.
- **AudioCapture Pipeline**: Captures microphone input from the selected or system-default device, resamples to mono **16-bit PCM at 16 kHz**, chunks frames for low-latency streaming, and computes normalized RMS audio levels emitted to the **Overlay Pill** for waveform rendering.
- **GeminiLiveClient (Transcription Transport)**: Connects to `gemini-3.5-transcribe-live` over WebSocket using the **Provider Credential**:
  - Sends session setup configuring `SMART` transcription mode and up to 1,000 **Custom Vocabulary** biasing terms.
  - Streams raw 16-bit PCM chunks continuously while `Listening`.
  - Emits incremental **Draft Transcript** updates as the model refines/self-corrects text.
  - On hotkey release (or 10:00 cap), signals end-of-audio, awaits finalization flush (with a bounded timeout so the UI never hangs), and yields the **Final Transcript**.
  - Provides a lightweight credential verification handshake used by Settings "Test Connection".
- **DeliveryService (ADR-0003, ADR-0005)**:
  - Executes **Text Insertion** via **Clipboard Preservation**: snapshots current clipboard text/state → writes **Final Transcript** to clipboard → triggers OS paste (`Cmd+V` on macOS, `Ctrl+V` on Windows/Linux) → waits a short configurable delay (~150 ms) → restores original clipboard if restoration is enabled.
  - Automatically falls back to **Fallback Delivery** (leaves **Final Transcript** in clipboard, skips restore, emits fallback notice to **Overlay Pill**) when paste fails, focus is unavailable, or the Linux Wayland environment lacks paste helper capabilities.
- **PlatformAdapter (Single OS Boundary Trait)**: Abstracts all OS-specific capabilities behind one trait implemented per desktop OS:
  - Global **Push-to-Talk Hotkey** press/release listener (plus Escape cancel detection during active session).
  - Microphone enumeration and PCM input stream.
  - Clipboard read/write and synthetic paste trigger.
  - **Provider Credential** CRUD backed by OS native keyring (macOS Keychain, Windows Credential Manager, Linux Secret Service / libsecret, with clear user warning if no Secret Service daemon is available on Linux).
  - Permission & environment capability probing (macOS Microphone + Accessibility status; Linux X11 vs Wayland + `evdev`/`ydotool`/`wtype` availability).
- **LocalStore (SQLite)**:
  - Stores **History Entry** records (`id`, `final_transcript`, `created_at_utc`, `duration_ms`, `was_partial`, `delivery_mode`). Never stores raw audio.
  - Stores **Custom Vocabulary** entries (`id`, `term`, `created_at_utc`) with a hard validation cap of 1,000 unique terms.
  - Stores non-secret app preferences (selected hotkey, microphone ID, clipboard restore toggle, history retention days defaulting to 30).
  - Runs retention pruning on startup and on retention setting changes.
  - Supports versioned JSON export/import of preferences + **Custom Vocabulary**, explicitly excluding **Provider Credential** and **History Entry** records.

### Desktop UI Surfaces (React + TypeScript + Tailwind CSS)
- **System Tray (Native)**: Shows current status, microphone picker submenu, "Open Settings & History", and "Quit".
- **Overlay Pill**: Transparent, rounded, non-focus-stealing floating bar positioned near the bottom center of the active display. Shows live waveform, streaming **Draft Transcript** (auto-scrolling to latest text), 9:30 countdown badge when near the 10-minute cap, and brief status/error/fallback banners.
- **Settings & History Window**:
  - **Onboarding / Permissions panel**: API key input + Test Connection, macOS Microphone/Accessibility checks with deep-link buttons to System Settings, Linux X11/Wayland capability status.
  - **General Settings**: **Push-to-Talk Hotkey** recorder, microphone selector, **Clipboard Preservation** restore toggle, history retention days.
  - **Custom Vocabulary**: Searchable list, add/edit/delete, live count indicator (`N / 1,000`), JSON export/import.
  - **History**: Searchable chronological list of **History Entry** items, one-click copy, individual delete, and "Clear All History".

## Testing Decisions

- **What makes a good test**: Tests verify observable external behavior (state transitions, emitted UI events, persisted SQLite rows, wire-level WebSocket messages, and clipboard/paste call sequences) rather than internal helper functions or private state.
- **Primary Testing Seam — Rust Core (`DictationController`)**:
  - Integration tests instantiate the real `DictationController`, real in-memory SQLite `LocalStore`, real `DeliveryService`, and real `GeminiLiveClient`.
  - Only two outermost boundaries are replaced with test doubles:
    1. **`FakePlatformAdapter`** implementing `PlatformAdapter`: feeds deterministic PCM audio frames, simulates hotkey press/release/cancel, records clipboard snapshot/write/paste/restore sequences, simulates paste failure or Linux Wayland capability limits, and backs an in-memory credential store.
    2. **Local Fake WebSocket Server**: binds to localhost and speaks the `gemini-3.5-transcribe-live` wire protocol, allowing tests to assert exact setup payloads (`SMART` mode + **Custom Vocabulary** terms) and PCM framing while deterministically injecting live **Draft Transcript** self-corrections, finalization flushes, 10-minute caps, and mid-stream socket drops.
  - **Key behaviors covered at this seam**: full Push-to-Talk happy path; **Draft Transcript** mid-sentence self-correction reflected in **Overlay Pill** events; **Clipboard Preservation** backup-paste-restore ordering and opt-out toggle; **Fallback Delivery** on paste failure or restricted Wayland; Escape abort (no insertion, no history); 9:30 warning & 10:00 auto-finalization; mid-stream disconnect salvaging partial text into clipboard + **History Entry**; 1,000-term **Custom Vocabulary** cap; 30-day history pruning; JSON export/import excluding **Provider Credential**.
- **Secondary Testing Seam — Desktop Frontend (Tauri IPC Boundary)**:
  - React component/integration tests mock only the Tauri `invoke` and event-subscription boundary to verify **Overlay Pill** rendering across all controller states, onboarding/connection test flows, **Custom Vocabulary** CRUD/limit UI, and **History Entry** search/copy/clear flows.
- **Prior Art**: Greenfield repository; these test suites establish the project's initial testing conventions.

## Out of Scope

- **Phase 2 Mobile apps** (iOS Swift custom keyboard extension & Android Kotlin IME) — the Rust Core is structured for UniFFI reuse, but mobile shells are deferred to Phase 2.
- **Web application / browser extension**.
- **Batch transcription (`gemini-3.5-transcribe`)** and long-form meeting recording / summarization / speaker diarization.
- **Secondary LLM post-processing** (custom rewrite prompts, tone switching per active app, translation mode).
- **Voice Snippets** (spoken text-expansion shortcuts) and IDE/codebase context-aware tagging.
- **Hands-free toggle trigger mode** (v1 is strictly Push-to-Talk).
- **Cloud accounts, backend servers, cross-device cloud sync, or audio file persistence**.
- **Zero-config native Wayland global key-up / paste across every compositor** (covered via progressive support and **Fallback Delivery** in ADR-0005).

## Further Notes

- **Canonical terminology**: All code identifiers, Tauri IPC commands, UI labels, and tests must follow [GLOSSARY.md](../../GLOSSARY.md).
- **Architecture Decision Records**: Key trade-offs are recorded in `docs/adr/0001` through `docs/adr/0005`.
- **Phase 2 readiness**: Keeping `PlatformAdapter` and `DictationController` free of Tauri types ensures the same Rust Core crate can be exposed to iOS and Android via UniFFI in Phase 2 without refactoring domain logic.
