import { useEffect, useState } from "react";
import {
  getSessionState,
  subscribeControllerEvents,
  type ControllerEvent,
  type SessionState,
} from "../ipc/tauri";

export function OverlayPill() {
  const [sessionState, setSessionState] = useState<SessionState>({
    kind: "idle",
  });
  const [draftTranscript, setDraftTranscript] = useState<string>("");
  const [audioLevel, setAudioLevel] = useState<number>(0);
  const [secondsRemaining, setSecondsRemaining] = useState<number | null>(null);

  useEffect(() => {
    let mounted = true;
    let unlisten: (() => void) | undefined;

    getSessionState()
      .then((initial) => {
        if (mounted) {
          setSessionState(initial);
        }
      })
      .catch(() => {
        // Ignore outside Tauri runtime.
      });

    subscribeControllerEvents((event: ControllerEvent) => {
      if (!mounted) return;
      switch (event.type) {
        case "state_changed":
          setSessionState(event.state);
          if (
            event.state.kind === "idle" ||
            event.state.kind === "connecting" ||
            event.state.kind === "error" ||
            event.state.kind === "fallback_notice"
          ) {
            setDraftTranscript("");
            setAudioLevel(0);
            setSecondsRemaining(null);
          }
          break;
        case "draft_transcript_updated":
          setDraftTranscript(event.draft_transcript);
          break;
        case "audio_level_updated": {
          const clamped = Number.isFinite(event.rms)
            ? Math.max(0, Math.min(1, event.rms))
            : 0;
          setAudioLevel(clamped);
          break;
        }
        case "session_warning":
          setSecondsRemaining(event.seconds_remaining);
          break;
        case "error_occurred":
          setSecondsRemaining(null);
          setSessionState({ kind: "error", message: event.message });
          break;
        case "fallback_delivery_noticed":
          setSessionState({
            kind: "fallback_notice",
            final_transcript: event.final_transcript,
            reason: event.reason,
          });
          break;
        default:
          break;
      }
    }).then((fn) => {
      if (mounted) {
        unlisten = fn;
      } else {
        fn();
      }
    });

    return () => {
      mounted = false;
      unlisten?.();
    };
  }, []);

  if (sessionState.kind === "idle") {
    return null;
  }

  const waveformBars = [0.4, 0.75, 1.0, 0.7, 0.45].map((scale) => {
    const heightPct = Math.max(20, Math.round(audioLevel * 100 * scale));
    return (
      <span
        key={`bar-${scale}`}
        className="w-1 rounded-full bg-emerald-400 transition-all duration-75"
        style={{ height: `${heightPct}%` }}
      />
    );
  });

  return (
    <div className="flex h-screen w-screen items-center justify-center bg-transparent p-2">
      <div
        role="status"
        aria-live="polite"
        className="flex w-full max-w-md items-center gap-3 rounded-full border border-slate-700/80 bg-slate-900/95 px-4 py-2.5 text-sm text-slate-100 shadow-xl backdrop-blur"
      >
        {sessionState.kind === "connecting" && (
          <>
            <span className="h-2.5 w-2.5 animate-pulse rounded-full bg-amber-400" />
            <span className="font-medium text-amber-200">Connecting...</span>
          </>
        )}

        {sessionState.kind === "listening" && (
          <>
            <div
              role="img"
              className="flex h-5 items-center gap-0.5"
              aria-label="Audio waveform"
              data-rms={audioLevel.toFixed(2)}
            >
              {waveformBars}
            </div>
            <div className="min-w-0 flex-1">
              <div className="flex items-center gap-2 text-xs font-medium text-emerald-300">
                <span>Listening</span>
                {secondsRemaining !== null && (
                  <span className="rounded bg-amber-500/20 px-1.5 py-0.5 text-amber-300">
                    {secondsRemaining}s left
                  </span>
                )}
              </div>
              <p className="truncate text-sm text-slate-100">
                {draftTranscript || "Speak now..."}
              </p>
            </div>
          </>
        )}

        {sessionState.kind === "finalizing" && (
          <>
            <span className="h-2.5 w-2.5 animate-pulse rounded-full bg-sky-400" />
            <span className="font-medium text-sky-200">Finalizing...</span>
          </>
        )}

        {sessionState.kind === "delivering" && (
          <>
            <span className="h-2.5 w-2.5 animate-pulse rounded-full bg-indigo-400" />
            <span className="font-medium text-indigo-200">Delivering...</span>
          </>
        )}

        {sessionState.kind === "error" && (
          <>
            <span className="h-2.5 w-2.5 rounded-full bg-rose-500" />
            <span className="truncate text-rose-200">
              {sessionState.message}
            </span>
          </>
        )}

        {sessionState.kind === "fallback_notice" && (
          <>
            <span className="h-2.5 w-2.5 rounded-full bg-amber-400" />
            <div className="min-w-0 flex-1">
              <div className="text-xs font-medium text-amber-300">
                Copied to clipboard (Fallback Delivery)
              </div>
              <p className="truncate text-xs text-slate-300">
                {sessionState.reason}
              </p>
            </div>
          </>
        )}
      </div>
    </div>
  );
}
