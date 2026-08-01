import * as React from "react"

import { Button } from "@/components/ui/button"
import {
  Card,
  CardAction,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import { Input } from "@/components/ui/input"
import {
  useModels,
  type DownloadState,
  type LocalModel,
} from "@/hooks/use-models"
import type { ModelEntry } from "@/lib/model-manifest"
import { CheckIcon, Trash2Icon, XIcon } from "lucide-react"

function formatGb(bytes: number) {
  return `${(bytes / 1_000_000_000).toFixed(1)} GB`
}

export function ModelsView() {
  const {
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
  } = useModels()

  const manifestIds = new Set(manifest?.models.map((entry) => entry.id) ?? [])
  const manifestFiles = new Set(
    manifest?.models.map((entry) => entry.file) ?? []
  )
  // URL downloads are keyed by file name; while one is in flight its stale
  // partial may still be in `locals`, so hide it from the local list.
  const customDownloads = Object.entries(downloads).filter(
    ([id, state]) => !manifestIds.has(id) && state.status === "downloading"
  )
  const customLocals = locals.filter(
    (local) =>
      !manifestFiles.has(local.file) &&
      downloads[local.file]?.status !== "downloading"
  )

  return (
    <div className="min-h-0 flex-1 overflow-y-auto">
      <div className="mx-auto flex w-full max-w-2xl flex-col gap-4 px-4 py-6">
        <div>
          <h2 className="text-sm font-medium">Models</h2>
          <p className="text-sm text-muted-foreground">
            Download a model to run inference locally. The active model is used
            for new sessions.
          </p>
        </div>
        {error && <p className="text-sm text-destructive">{error}</p>}
        <AddModelFromUrl onDownload={downloadFromUrl} />
        {customLocals.map((local) => (
          <CustomModelCard
            key={local.file}
            local={local}
            isActive={activeFile === local.file}
            onDelete={() => remove(local.file)}
            onActivate={() => setActive(local.file)}
          />
        ))}
        {customDownloads.map(([file, state]) => (
          <Card size="sm" key={file}>
            <CardHeader>
              <CardTitle className="truncate font-mono text-sm">
                {file}
              </CardTitle>
              <CardAction>
                <Button
                  variant="outline"
                  size="xs"
                  onClick={() => cancel(file)}
                >
                  <XIcon data-icon="inline-start" />
                  Cancel
                </Button>
              </CardAction>
            </CardHeader>
            <CardContent>
              <DownloadProgress state={state} />
            </CardContent>
          </Card>
        ))}
        {(customLocals.length > 0 || customDownloads.length > 0) && (
          <div className="mt-2">
            <h2 className="text-sm font-medium">Catalog</h2>
            <p className="text-sm text-muted-foreground">
              Curated models that work well on Apple Silicon.
            </p>
          </div>
        )}
        {manifest?.models.map((entry) => (
          <ModelCard
            key={entry.id}
            entry={entry}
            local={locals.find((local) => local.file === entry.file)}
            downloadState={downloads[entry.id]}
            isActive={activeFile === entry.file}
            onDownload={() => download(entry.id)}
            onCancel={() => cancel(entry.id)}
            onDelete={() => remove(entry.file)}
            onActivate={() => setActive(entry.file)}
          />
        ))}
      </div>
    </div>
  )
}

function AddModelFromUrl({
  onDownload,
}: {
  onDownload: (url: string) => Promise<unknown>
}) {
  const [url, setUrl] = React.useState("")
  const [error, setError] = React.useState<string | null>(null)

  function submit() {
    const trimmed = url.trim()
    if (!trimmed) return
    setError(null)
    // Resolves when the download finishes; rejections (invalid URL, HTTP
    // errors) surface here since URL downloads have no catalog card.
    onDownload(trimmed).catch((err) => setError(String(err)))
    setUrl("")
  }

  return (
    <Card size="sm">
      <CardHeader>
        <CardTitle>Add from URL</CardTitle>
        <CardDescription>
          Paste a direct link to a .gguf file, e.g. a Hugging Face download URL.
        </CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-2">
        <form
          className="flex items-center gap-2"
          onSubmit={(event) => {
            event.preventDefault()
            submit()
          }}
        >
          <Input
            value={url}
            onChange={(event) => {
              setUrl(event.target.value)
              setError(null)
            }}
            placeholder="https://huggingface.co/…/model-Q4_K_M.gguf"
            className="font-mono text-xs"
          />
          <Button type="submit" size="xs" disabled={!url.trim()}>
            Download
          </Button>
        </form>
        {error && <p className="text-xs text-destructive">{error}</p>}
      </CardContent>
    </Card>
  )
}

function CustomModelCard({
  local,
  isActive,
  onDelete,
  onActivate,
}: {
  local: LocalModel
  isActive: boolean
  onDelete: () => void
  onActivate: () => void
}) {
  return (
    <Card size="sm">
      <CardHeader>
        <CardTitle className="flex items-center gap-2 truncate font-mono text-sm">
          {local.file}
          {isActive && (
            <span className="rounded-full bg-primary px-2 py-0.5 font-sans text-[10px] font-semibold tracking-normal text-primary-foreground">
              Active
            </span>
          )}
        </CardTitle>
        <CardAction className="flex items-center gap-2">
          {!local.partial && !isActive && (
            <Button variant="outline" size="xs" onClick={onActivate}>
              <CheckIcon data-icon="inline-start" />
              Use
            </Button>
          )}
          <Button variant="ghost" size="icon-xs" onClick={onDelete}>
            <Trash2Icon />
            <span className="sr-only">Delete</span>
          </Button>
        </CardAction>
      </CardHeader>
      <CardContent>
        <p className="font-mono text-xs text-muted-foreground">
          {local.partial
            ? `partial download · ${formatGb(local.sizeBytes)} — paste its URL again to resume`
            : formatGb(local.sizeBytes)}
        </p>
      </CardContent>
    </Card>
  )
}

function ModelCard({
  entry,
  local,
  downloadState,
  isActive,
  onDownload,
  onCancel,
  onDelete,
  onActivate,
}: {
  entry: ModelEntry
  local?: LocalModel
  downloadState?: DownloadState
  isActive: boolean
  onDownload: () => void
  onCancel: () => void
  onDelete: () => void
  onActivate: () => void
}) {
  const downloading = downloadState?.status === "downloading"
  const downloaded = Boolean(local && !local.partial)
  const partial = Boolean(local?.partial) && !downloading

  return (
    <Card size="sm">
      <CardHeader>
        <CardTitle className="flex items-center gap-2">
          {entry.name}
          {isActive && (
            <span className="rounded-full bg-primary px-2 py-0.5 font-sans text-[10px] font-semibold tracking-normal text-primary-foreground">
              Active
            </span>
          )}
        </CardTitle>
        <CardDescription>{entry.description}</CardDescription>
        <CardAction className="flex items-center gap-2">
          {downloading ? (
            <Button variant="outline" size="xs" onClick={onCancel}>
              <XIcon data-icon="inline-start" />
              Cancel
            </Button>
          ) : downloaded ? (
            <>
              {!isActive && (
                <Button variant="outline" size="xs" onClick={onActivate}>
                  <CheckIcon data-icon="inline-start" />
                  Use
                </Button>
              )}
              <Button variant="ghost" size="icon-xs" onClick={onDelete}>
                <Trash2Icon />
                <span className="sr-only">Delete</span>
              </Button>
            </>
          ) : (
            <>
              <Button size="xs" onClick={onDownload}>
                {partial ? "Resume" : "Download"}
              </Button>
              {partial && (
                <Button variant="ghost" size="icon-xs" onClick={onDelete}>
                  <Trash2Icon />
                  <span className="sr-only">Delete partial download</span>
                </Button>
              )}
            </>
          )}
        </CardAction>
      </CardHeader>
      <CardContent className="flex flex-col gap-3">
        <p className="font-mono text-xs text-muted-foreground">
          {entry.quant} · {formatGb(entry.sizeBytes)} · needs {entry.minRamGb}{" "}
          GB RAM
        </p>
        {downloading && downloadState && (
          <DownloadProgress state={downloadState} />
        )}
        {downloadState?.status === "failed" && (
          <p className="text-xs text-destructive">
            Download failed: {downloadState.error ?? "unknown error"}
          </p>
        )}
      </CardContent>
    </Card>
  )
}

function DownloadProgress({ state }: { state: DownloadState }) {
  const percent =
    state.total > 0 ? Math.min(100, (state.downloaded / state.total) * 100) : 0
  return (
    <div className="flex items-center gap-3">
      <div className="h-1.5 flex-1 overflow-hidden rounded-full bg-muted">
        <div
          className="h-full rounded-full bg-primary transition-[width] duration-300"
          style={{ width: `${percent}%` }}
        />
      </div>
      <span className="w-24 text-right font-mono text-xs text-muted-foreground">
        {formatGb(state.downloaded)} / {formatGb(state.total)}
      </span>
    </div>
  )
}
