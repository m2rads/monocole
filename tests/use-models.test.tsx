import { act, renderHook, waitFor } from "@testing-library/react"
import { describe, expect, it } from "vitest"

import { useModels, type LocalModel } from "@/hooks/use-models"
import type { ModelManifest } from "@/lib/model-manifest"

import { emitTauriEvent, invokeCalls, invokeMock } from "./tauri-mocks"

const manifest: ModelManifest = {
  version: 1,
  models: [
    {
      id: "test-model",
      name: "Test model",
      description: "A model for tests",
      repo: "test/repo",
      file: "test.gguf",
      url: "https://huggingface.co/test/repo/resolve/main/test.gguf",
      quant: "Q4_K_M",
      sizeBytes: 1000,
      minRamGb: 8,
      sha256: null,
    },
  ],
}

function mockBackend(options?: {
  locals?: LocalModel[]
  active?: string | null
}) {
  invokeMock.mockImplementation(async (cmd) => {
    switch (cmd) {
      case "get_model_manifest":
        return manifest as never
      case "list_local_models":
        return (options?.locals ?? []) as never
      case "get_active_model":
        return (options?.active ?? null) as never
      default:
        return undefined as never
    }
  })
}

describe("useModels", () => {
  it("loads the manifest, local models, and active model on mount", async () => {
    mockBackend({
      locals: [{ file: "test.gguf", sizeBytes: 1000, partial: false }],
      active: "test.gguf",
    })
    const { result } = renderHook(() => useModels())

    await waitFor(() => expect(result.current.manifest).not.toBeNull())
    expect(result.current.manifest?.models[0].id).toBe("test-model")
    expect(result.current.locals).toHaveLength(1)
    expect(result.current.activeFile).toBe("test.gguf")
  })

  it("tracks download progress from events and refreshes on completion", async () => {
    mockBackend()
    const { result } = renderHook(() => useModels())
    await waitFor(() => expect(result.current.manifest).not.toBeNull())

    act(() => result.current.download("test-model"))
    expect(invokeCalls("download_model")[0][1]).toEqual({ id: "test-model" })

    act(() => {
      emitTauriEvent("model-download", {
        id: "test-model",
        status: "downloading",
        downloaded: 250,
        total: 1000,
      })
    })
    expect(result.current.downloads["test-model"]).toMatchObject({
      status: "downloading",
      downloaded: 250,
      total: 1000,
    })

    // Completion refreshes the local list, which now contains the file.
    mockBackend({
      locals: [{ file: "test.gguf", sizeBytes: 1000, partial: false }],
    })
    act(() => {
      emitTauriEvent("model-download", {
        id: "test-model",
        status: "completed",
        downloaded: 1000,
        total: 1000,
      })
    })
    await waitFor(() => expect(result.current.locals).toHaveLength(1))
  })

  it("surfaces failed downloads", async () => {
    mockBackend()
    const { result } = renderHook(() => useModels())
    await waitFor(() => expect(result.current.manifest).not.toBeNull())

    act(() => {
      emitTauriEvent("model-download", {
        id: "test-model",
        status: "failed",
        downloaded: 0,
        total: 1000,
        error: "HTTP 404",
      })
    })
    expect(result.current.downloads["test-model"]).toMatchObject({
      status: "failed",
      error: "HTTP 404",
    })
  })

  it("activates a model", async () => {
    mockBackend({
      locals: [{ file: "test.gguf", sizeBytes: 1000, partial: false }],
    })
    const { result } = renderHook(() => useModels())
    await waitFor(() => expect(result.current.locals).toHaveLength(1))

    await act(() => result.current.setActive("test.gguf"))

    expect(invokeCalls("set_active_model")[0][1]).toEqual({
      file: "test.gguf",
    })
    expect(result.current.activeFile).toBe("test.gguf")
  })

  it("deletes a model and refreshes", async () => {
    mockBackend({
      locals: [{ file: "test.gguf", sizeBytes: 1000, partial: false }],
    })
    const { result } = renderHook(() => useModels())
    await waitFor(() => expect(result.current.locals).toHaveLength(1))

    mockBackend({ locals: [] })
    await act(() => result.current.remove("test.gguf"))

    expect(invokeCalls("delete_model")[0][1]).toEqual({ file: "test.gguf" })
    expect(result.current.locals).toHaveLength(0)
  })
})
