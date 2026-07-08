import * as React from "react"

import { invoke } from "@tauri-apps/api/core"
import { listen } from "@tauri-apps/api/event"

import { getModelManifest, type ModelManifest } from "@/lib/model-manifest"

export type LocalModel = {
  file: string
  sizeBytes: number
  partial: boolean
}

export type DownloadStatus = "downloading" | "completed" | "cancelled" | "failed"

export type DownloadState = {
  status: DownloadStatus
  downloaded: number
  total: number
  error?: string
}

type DownloadEvent = DownloadState & { id: string }

export function useModels() {
  const [manifest, setManifest] = React.useState<ModelManifest | null>(null)
  const [locals, setLocals] = React.useState<LocalModel[]>([])
  const [activeFile, setActiveFile] = React.useState<string | null>(null)
  const [downloads, setDownloads] = React.useState<
    Record<string, DownloadState>
  >({})
  const [error, setError] = React.useState<string | null>(null)

  const refresh = React.useCallback(async () => {
    setLocals(await invoke<LocalModel[]>("list_local_models"))
    setActiveFile(await invoke<string | null>("get_active_model"))
  }, [])

  React.useEffect(() => {
    getModelManifest().then(setManifest).catch((err) => setError(String(err)))
    refresh().catch((err) => setError(String(err)))

    const unlisten = listen<DownloadEvent>("model-download", (event) => {
      const { id, ...state } = event.payload
      setDownloads((prev) => ({ ...prev, [id]: state }))
      if (state.status !== "downloading") {
        refresh().catch((err) => setError(String(err)))
      }
    })
    return () => {
      unlisten.then((fn) => fn())
    }
  }, [refresh])

  const download = React.useCallback((id: string) => {
    setDownloads((prev) => {
      const next = { ...prev }
      delete next[id]
      return next
    })
    invoke("download_model", { id }).catch(() => {
      // Failure details arrive via the model-download event.
    })
  }, [])

  // Progress events for URL downloads are keyed by the file name the backend
  // derives from the URL. Rejects on invalid URLs and download failures, so
  // the caller can surface the error next to the input.
  const downloadFromUrl = React.useCallback((url: string) => {
    return invoke("download_model_from_url", { url })
  }, [])

  const cancel = React.useCallback((id: string) => {
    invoke("cancel_download", { id }).catch((err) => setError(String(err)))
  }, [])

  const remove = React.useCallback(
    async (file: string) => {
      try {
        await invoke("delete_model", { file })
        await refresh()
      } catch (err) {
        setError(String(err))
      }
    },
    [refresh]
  )

  const setActive = React.useCallback(
    async (file: string | null) => {
      try {
        await invoke("set_active_model", { file })
        setActiveFile(file)
      } catch (err) {
        setError(String(err))
      }
    },
    []
  )

  return {
    manifest,
    locals,
    activeFile,
    downloads,
    error,
    download,
    downloadFromUrl,
    cancel,
    remove,
    setActive,
  }
}
