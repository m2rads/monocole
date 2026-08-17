import { invoke } from "@tauri-apps/api/core"

// Mirrors ModelManifest / ModelEntry in src-tauri/src/manifest.rs.
// TODO(config-server): today this is the manifest bundled into the app
// binary; later the backend will serve remote > cached > bundled (see the
// plan in manifest.rs) without this interface changing.
/** What a model is for. Mirrors ModelKind in manifest.rs. */
export type ModelKind = "chat" | "stt"

export type ModelEntry = {
  /** "chat" models are the ones the user picks between; "stt" is whisper's
   *  speech model, fetched once and never chosen. */
  kind: ModelKind
  id: string
  name: string
  description: string
  repo: string
  file: string
  url: string
  quant: string
  sizeBytes: number
  minRamGb: number
  sha256: string | null
}

export type ModelManifest = {
  version: number
  models: ModelEntry[]
}

export function getModelManifest(): Promise<ModelManifest> {
  return invoke<ModelManifest>("get_model_manifest")
}

/**
 * The models a user can choose to chat with.
 *
 * The catalog also carries whisper's speech model, which shares the download
 * machinery but must never appear as a selectable chat model — activating it
 * would point llama-server at a file it cannot read.
 */
export function chatModels(manifest: ModelManifest): ModelEntry[] {
  return manifest.models.filter((model) => model.kind !== "stt")
}

/** The speech model, if the catalog lists one. */
export function sttModel(manifest: ModelManifest): ModelEntry | undefined {
  return manifest.models.find((model) => model.kind === "stt")
}
