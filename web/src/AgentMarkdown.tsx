import ReactMarkdown from 'react-markdown'
import rehypeSanitize from 'rehype-sanitize'
import remarkGfm from 'remark-gfm'

export default function AgentMarkdown({ text }: { text: string }) {
  return (
    <div className="agent-output__markdown">
      <ReactMarkdown
        components={{
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
