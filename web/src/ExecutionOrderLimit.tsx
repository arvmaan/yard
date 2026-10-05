import { MAX_EXECUTION_ORDER_BYTES } from './orderTemplates'

const formatBytes = (bytes: number) => bytes.toLocaleString('en-US')

/** Live UTF-8 byte count of the order Yard will send, against its limit. */
export function ExecutionOrderByteCount({
  bytes,
  id,
}: {
  bytes: number
  id: string
}) {
  return (
    <small
      className="chat-composer__limit"
      data-over={bytes > MAX_EXECUTION_ORDER_BYTES ? true : undefined}
      id={id}
    >
      {formatBytes(bytes)} / {formatBytes(MAX_EXECUTION_ORDER_BYTES)} bytes
    </small>
  )
}

export function ExecutionOrderLimitError({ bytes }: { bytes: number }) {
  if (bytes <= MAX_EXECUTION_ORDER_BYTES) return null
  return (
    <p className="chat-composer__limit-error" role="alert">
      This order is {formatBytes(bytes)} bytes including Yard&apos;s exit
      contract. Yard accepts at most {formatBytes(MAX_EXECUTION_ORDER_BYTES)}{' '}
      bytes; shorten it to send.
    </p>
  )
}
