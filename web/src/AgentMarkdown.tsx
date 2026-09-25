import { Check, Copy } from 'lucide-react'
import {
  isValidElement,
  useEffect,
  useRef,
  useState,
  type ReactNode,
} from 'react'
import ReactMarkdown from 'react-markdown'
import rehypeSanitize from 'rehype-sanitize'
import remarkGfm from 'remark-gfm'

type CopyState = 'copied' | 'error' | 'idle'

function nodeText(node: ReactNode): string {
  if (typeof node === 'string' || typeof node === 'number') {
    return String(node)
  }
  if (Array.isArray(node)) {
    return node.map(nodeText).join('')
  }
  if (isValidElement<{ children?: ReactNode }>(node)) {
    return nodeText(node.props.children)
  }
  return ''
}

async function copyText(text: string) {
  try {
    await navigator.clipboard.writeText(text)
    return
  } catch {
    const textarea = document.createElement('textarea')
    textarea.value = text
    textarea.setAttribute('readonly', '')
    textarea.style.position = 'fixed'
    textarea.style.opacity = '0'
    document.body.append(textarea)
    textarea.select()
    const copied = document.execCommand('copy')
    textarea.remove()
    if (!copied) throw new Error('Clipboard access is unavailable')
  }
}

function CopyableCodeBlock({ children }: { children?: ReactNode }) {
  const [copyState, setCopyState] = useState<CopyState>('idle')
  const resetTimer = useRef<number | null>(null)
  const text = nodeText(children).replace(/\n$/, '')

  useEffect(
    () => () => {
      if (resetTimer.current !== null) {
        window.clearTimeout(resetTimer.current)
      }
    },
    [],
  )

  const handleCopy = async () => {
    try {
      await copyText(text)
      setCopyState('copied')
    } catch {
      setCopyState('error')
    }
    if (resetTimer.current !== null) {
      window.clearTimeout(resetTimer.current)
    }
    resetTimer.current = window.setTimeout(() => {
      setCopyState('idle')
      resetTimer.current = null
    }, 1800)
  }

  const label = copyState === 'copied'
    ? 'Copied'
    : copyState === 'error'
      ? 'Copy failed'
      : 'Copy code'

  return (
    <div className="agent-output__code-block">
      <button
        aria-label={`${label} to clipboard`}
        className="agent-output__copy-code"
        data-state={copyState}
        disabled={!text}
        onClick={() => void handleCopy()}
        title={label}
        type="button"
      >
        {copyState === 'copied' ? (
          <Check aria-hidden="true" size={13} />
        ) : (
          <Copy aria-hidden="true" size={13} />
        )}
        <span>{label}</span>
      </button>
      <pre>{children}</pre>
    </div>
  )
}

export default function AgentMarkdown({ text }: { text: string }) {
  return (
    <div className="agent-output__markdown">
      <ReactMarkdown
        components={{
          pre: ({ children }) => (
            <CopyableCodeBlock>{children}</CopyableCodeBlock>
          ),
          img: ({ alt }) => (
            <span className="agent-output__markdown-image">
              {alt || 'Image'}
            </span>
          ),
        }}
        rehypePlugins={[rehypeSanitize]}
        remarkPlugins={[remarkGfm]}
        skipHtml
      >
        {text}
      </ReactMarkdown>
    </div>
  )
}
