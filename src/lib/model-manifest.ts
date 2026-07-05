import { invoke } from "@tauri-apps/api/core"

// Mirrors ModelManifest / ModelEntry in src-tauri/src/manifest.rs.
// TODO(config-server): today this is the manifest bundled into the app
// binary; later the backend will serve remote > cached > bundled (see the
// plan in manifest.rs) without this interface changing.
export type ModelEntry = {
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
