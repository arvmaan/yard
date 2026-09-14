export const TERMINAL_INPUT_CHUNK_BYTES = 4 * 1024

const encoder = new TextEncoder()

export function chunkTerminalInput(text: string): string[] {
  const chunks: string[] = []
  let chunk = ''
  let chunkBytes = 0

  for (const character of text) {
    const characterBytes = encoder.encode(character).byteLength
    if (
      chunk &&
      chunkBytes + characterBytes > TERMINAL_INPUT_CHUNK_BYTES
    ) {
      chunks.push(chunk)
      chunk = ''
      chunkBytes = 0
    }
    chunk += character
    chunkBytes += characterBytes
  }

  if (chunk) chunks.push(chunk)
  return chunks
}
