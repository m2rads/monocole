import * as React from "react"

import { invoke } from "@tauri-apps/api/core"

export type RecorderStatus = "idle" | "recording" | "transcribing"

/**
 * Records from the Mac's microphone and transcribes it with whisper.
 *
 * Capture happens in Rust, not here: WKWebView does not expose
 * `navigator.mediaDevices` to embedded content on macOS, so `getUserMedia` is
 * absent rather than merely permission-gated and no browser recorder can work
 * inside the app window.
 *
 * Deliberately separate from the monocle's voice path, while reaching the same
 * speech model — a blank transcript here means whisper, and a blank transcript
 * only there means everything upstream of it.
 */
export function useRecorder(onTranscript: (text: string) => void) {
  const [status, setStatus] = React.useState<RecorderStatus>("idle")
  const [error, setError] = React.useState<string | null>(null)
  const [seconds, setSeconds] = React.useState(0)

  // Tick the elapsed counter while recording.
  React.useEffect(() => {
    if (status !== "recording") return
    const started = Date.now()
    const timer = setInterval(
      () => setSeconds((Date.now() - started) / 1000),
      100
    )
    return () => clearInterval(timer)
  }, [status])

  const start = React.useCallback(async () => {
    setError(null)
    setSeconds(0)
    try {
      await invoke("start_recording")
      setStatus("recording")
    } catch (err) {
      setError(String(err))
    }
  }, [])

  const stop = React.useCallback(async () => {
    setStatus("transcribing")
    try {
      const text = await invoke<string>("stop_recording")
      if (text) {
        onTranscript(text)
      } else {
        // Whisper labels silence rather than failing on it, and Rust turns
        // those labels into nothing — so an empty string here is the one
        // signal that the microphone heard no words, and it has to be said
        // out loud or the app looks like it ignored you.
        setError("Couldn't capture anything, try speaking louder.")
      }
    } catch (err) {
      setError(String(err))
    } finally {
      setStatus("idle")
    }
  }, [onTranscript])

  return { status, error, seconds, start, stop }
}
