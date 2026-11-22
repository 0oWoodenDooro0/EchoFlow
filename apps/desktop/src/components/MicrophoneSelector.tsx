import { useCallback, useEffect, useState } from "react";
import {
  getSelectedMicrophone,
  listAudioDevices,
  selectMicrophone,
  type AudioDeviceInfo,
} from "../ipc/tauri";

export function MicrophoneSelector() {
  const [devices, setDevices] = useState<AudioDeviceInfo[]>([]);
  const [selectedId, setSelectedId] = useState<string>("");
  const [loading, setLoading] = useState<boolean>(true);
  const [error, setError] = useState<string | null>(null);

  const loadDevices = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const [enumerated, current] = await Promise.all([
        listAudioDevices(),
        getSelectedMicrophone(),
      ]);
      setDevices(enumerated);
      setSelectedId(current ?? "");
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadDevices();
  }, [loadDevices]);

  const handleChange = async (nextValue: string) => {
    const nextDeviceId = nextValue === "" ? null : nextValue;
    setSelectedId(nextValue);
    setError(null);
    try {
      await selectMicrophone(nextDeviceId);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  return (
    <div className="space-y-2 rounded-lg border border-slate-800 bg-slate-900/60 p-4">
      <div className="flex items-center justify-between gap-3">
        <label
          htmlFor="microphone-select"
          className="text-sm font-medium text-slate-200"
        >
          Microphone Input Device
        </label>
        <button
          type="button"
          onClick={() => void loadDevices()}
          disabled={loading}
          className="rounded border border-slate-700 px-2.5 py-1 text-xs font-medium text-slate-300 transition hover:bg-slate-800 disabled:opacity-50"
        >
          Refresh
        </button>
      </div>

      {devices.length === 0 && !loading ? (
        <p className="text-xs text-amber-300">
          No microphone input devices detected. Connect a microphone and click
          Refresh.
        </p>
      ) : (
        <select
          id="microphone-select"
          value={selectedId}
          disabled={loading}
          onChange={(e) => void handleChange(e.target.value)}
          style={{ colorScheme: "dark" }}
          className="w-full rounded-md border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-slate-100 focus:border-emerald-500 focus:outline-none"
        >
          <option value="" className="bg-slate-900 text-slate-100">
            System Default Microphone
          </option>
          {devices.map((device) => (
            <option
              key={device.id}
              value={device.id}
              className="bg-slate-900 text-slate-100"
            >
              {device.is_default ? `${device.name} (Default)` : device.name}
            </option>
          ))}
        </select>
      )}

      {error && <p className="text-xs text-rose-400">{error}</p>}
    </div>
  );
}
