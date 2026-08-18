import {
  useEffect,
  useMemo,
  useRef,
  useState,
} from 'react'
import { createPortal } from 'react-dom'
import DOMPurify from 'dompurify'
import {
  Code2,
  Eye,
  FileCode2,
  LoaderCircle,
  X,
} from 'lucide-react'
import ReactMarkdown from 'react-markdown'
import rehypeSanitize from 'rehype-sanitize'
import remarkGfm from 'remark-gfm'
import { fetchArtifactContent } from './api'
import type { Artifact } from './types'
import { useModalDialog } from './useModalDialog'

interface ArtifactInspectorProps {
  artifact: Artifact
  onClose: () => void
  returnFocus: HTMLElement | null
}

export function ArtifactInspector({
  artifact,
  onClose,
  returnFocus,
}: ArtifactInspectorProps) {
  const [content, setContent] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [view, setView] = useState<'preview' | 'source'>('preview')
  const dialogRef = useRef<HTMLElement>(null)
  useModalDialog({
    dialogRef,
    onClose,
    returnFocus,
  })

  useEffect(() => {
    const controller = new AbortController()
    setContent(null)
    setError(null)
    setView('preview')
    fetchArtifactContent(
      artifact.project_id,
      artifact.assignment_id,
      artifact.id,
      controller.signal,
    )
      .then((result) => setContent(result.content))
      .catch((caught: unknown) => {
        if (caught instanceof DOMException && caught.name === 'AbortError') {
          return
        }
        setError(
          caught instanceof Error
            ? caught.message
            : 'Artifact content is unavailable',
        )
      })
    return () => controller.abort()
  }, [artifact])

  const sandboxedHtml = useMemo(
    () =>
      artifact.kind === 'html' && content !== null
        ? buildSandboxedHtml(content)
        : null,
    [artifact.kind, content],
  )

  return createPortal(
    <div className="modal-backdrop artifact-backdrop" role="presentation">
      <section
        aria-labelledby="artifact-inspector-title"
        aria-modal="true"
        className="artifact-inspector"
        ref={dialogRef}
        role="dialog"
      >
        <header className="artifact-inspector__header">
          <span className="artifact-inspector__icon">
            <FileCode2 aria-hidden="true" size={20} />
          </span>
          <div>
            <p className="eyebrow">{artifact.kind} artifact</p>
            <h2 id="artifact-inspector-title">{artifact.display_name}</h2>
          </div>
          <button
            aria-label="Close artifact"
            autoFocus
            className="icon-button"
            onClick={onClose}
            title="Close"
            type="button"
          >
            <X aria-hidden="true" size={18} />
          </button>
        </header>

        <div className="artifact-inspector__toolbar">
          <dl>
            <div>
              <dt>Type</dt>
              <dd>{artifact.media_type}</dd>
            </div>
            <div>
              <dt>Size</dt>
              <dd>{formatBytes(artifact.byte_size)}</dd>
            </div>
            <div>
              <dt>SHA-256</dt>
              <dd title={artifact.sha256}>{artifact.sha256.slice(0, 12)}</dd>
            </div>
          </dl>
          <div aria-label="Artifact view" className="segmented-control">
            <button
              aria-pressed={view === 'preview'}
              onClick={() => setView('preview')}
              type="button"
            >
              <Eye aria-hidden="true" size={15} />
              Preview
            </button>
            <button
              aria-pressed={view === 'source'}
              onClick={() => setView('source')}
              type="button"
            >
              <Code2 aria-hidden="true" size={15} />
              Source
            </button>
          </div>
        </div>

        <div className="artifact-inspector__body">
          {content === null && !error ? (
            <div className="artifact-inspector__state" role="status">
              <LoaderCircle
                aria-hidden="true"
                className="status-spin"
                size={20}
              />
              <span>Loading artifact</span>
            </div>
          ) : null}
          {error ? (
            <div className="artifact-inspector__state" role="alert">
              <FileCode2 aria-hidden="true" size={20} />
              <span>{error}</span>
            </div>
          ) : null}
          {content !== null && view === 'source' ? (
            <pre className="artifact-source">
              <code>{content}</code>
            </pre>
          ) : null}
          {content !== null &&
          view === 'preview' &&
          artifact.kind === 'markdown' ? (
            <article className="artifact-markdown">
              <ReactMarkdown
                components={{
                  a: ({ children }) => <span>{children}</span>,
                  img: ({ alt }) => (
                    <span className="artifact-markdown__image">
                      {alt || 'Image'}
                    </span>
                  ),
                }}
                rehypePlugins={[rehypeSanitize]}
                remarkPlugins={[remarkGfm]}
                skipHtml
              >
                {content}
              </ReactMarkdown>
            </article>
          ) : null}
          {sandboxedHtml !== null && view === 'preview' ? (
            <iframe
              className="artifact-html"
              referrerPolicy="no-referrer"
              sandbox=""
              srcDoc={sandboxedHtml}
              title={`${artifact.display_name} preview`}
            />
          ) : null}
        </div>
      </section>
    </div>,
    document.body,
  )
}

function buildSandboxedHtml(content: string) {
  const sanitized = DOMPurify.sanitize(content, {
    ALLOW_DATA_ATTR: false,
    FORBID_TAGS: [
      'base',
      'button',
      'embed',
      'form',
      'iframe',
      'input',
      'link',
      'meta',
      'object',
      'script',
      'select',
      'textarea',
    ],
    WHOLE_DOCUMENT: true,
  })
  const parsed = new DOMParser().parseFromString(sanitized, 'text/html')
  parsed.querySelectorAll('img, audio, video, source').forEach((element) => {
    const source = element.getAttribute('src')
    if (!source?.startsWith('data:')) element.remove()
  })
  const policy = parsed.createElement('meta')
  policy.httpEquiv = 'Content-Security-Policy'
  policy.content =
    "default-src 'none'; img-src data:; media-src data:; font-src data:; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'"
  parsed.head.prepend(policy)
  return `<!doctype html>${parsed.documentElement.outerHTML}`
}

function formatBytes(value: number) {
  return value < 1024 ? `${value} B` : `${Math.ceil(value / 1024)} KiB`
}
