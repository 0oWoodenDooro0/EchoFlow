# EchoFlow Coding Standards

Standards enforced during code review. Mechanical checks (`cargo fmt`, `cargo clippy -D warnings`, `biome lint`, `tsc`) are enforced by CI; this file covers judgement calls that linters cannot catch.

## Architecture & Boundaries

- **Core stays Tauri-free (ADR-0001)**: `crates/echoflow-core` must not depend on `tauri` or OS GUI/audio/keyring crates. Platform integration belongs in `apps/desktop/src-tauri` behind `PlatformAdapter` traits.
- **Single source of truth**: Do not duplicate domain validation rules, capability probes, or IPC type definitions across modules or between Rust and TypeScript. Keep one authoritative implementation and delegate to it.

## Clarity & Decomposition

- **Keep orchestration handlers focused**: Break long state-machine or event-loop handlers (e.g., >100 lines with multiple phases) into named helper methods per phase rather than nesting deeply in one function.
- **No silent degradation**: Every fallback path (missing microphone, unavailable Secret Service, unsupported display-server insertion, mid-stream disconnect salvage) must emit a structured `DEGRADED` log and surface actionable user feedback when relevant.

## Domain Language

- **Use canonical terms from `GLOSSARY.md`**: Prefer `Dictation Session`, `Push-to-Talk Hotkey`, `Draft Transcript`, `Final Transcript`, `Text Insertion`, `Fallback Delivery`, `Clipboard Preservation`, `Custom Vocabulary`, `History Entry`, and `Provider Credential` in code, IPC names, and UI copy. Avoid avoided synonyms listed in `GLOSSARY.md`.
