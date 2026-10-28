// Images pasted or dropped into a web terminal. The agent runs on the Yard
// host and cannot read this browser's clipboard, so the image is uploaded
// to Yard and its absolute path on the host is typed into the pane (as a
// bracketed paste and a space, never Enter), where Claude Code and Codex
// pick it up as an attached image.

import { YardApiError, type PastedTerminalImage } from './api'
import { bracketedTerminalPaste, formatTerminalBytes } from './terminalProtocol'

export const PASTE_IMAGE_TYPES: readonly string[] = [
  'image/png',
  'image/jpeg',
  'image/gif',
  'image/webp',
]

export interface TransferImages {
  /** Images Yard accepts, in clipboard order. */
  images: File[]
  /** MIME types of images it does not (for example image/tiff). */
  unsupported: string[]
  /** Every file that is not an accepted image (including unsupported images). */
  skipped: File[]
}

/**
 * The image files in a paste or drop. `items` is preferred (it is what a
 * clipboard paste fills); `files` covers drops that only list files.
 */
export function transferImages(
  data: Pick<DataTransfer, 'items' | 'files'> | null | undefined,
): TransferImages {
  const result: TransferImages = { images: [], unsupported: [], skipped: [] }
  if (!data) return result
  const files: File[] = []
  const items = data.items ? Array.from(data.items) : []
  for (const item of items) {
    if (item.kind !== 'file') continue
    const file = item.getAsFile()
    if (file) files.push(file)
  }
  if (files.length === 0 && data.files) files.push(...Array.from(data.files))
  for (const file of files) {
    const type = file.type.toLowerCase()
    if (PASTE_IMAGE_TYPES.includes(type)) {
      result.images.push(file)
      continue
    }
    result.skipped.push(file)
    if (type.startsWith('image/')) result.unsupported.push(type)
  }
  return result
}

/** A host path as typed text: spaces and backslashes escaped. */
export function terminalPathText(path: string) {
  return path.replace(/[\\ ]/g, (character) => `\\${character}`)
}

/**
 * Pause between the `terminal.input` messages of one image paste. Each path
 * (and the text after them) must reach the agent as its own paste: Claude
 * Code gathers paste chunks that arrive close together before it checks for
 * an image path, and Codex attaches a path only when it is the whole paste.
 */
export const IMAGE_PASTE_GAP_MS = 150

/**
 * Control input that must reach the pane at once, even while an image paste
 * is uploading (typing otherwise waits behind it so the image path lands
 * first): a bare Esc, or a single C0 control character such as Ctrl-C,
 * Ctrl-D, Ctrl-Z or Ctrl-\. Enter, Tab and Backspace are editing keys and
 * keep their order behind the paste.
 */
export function isTerminalControlInput(text: string): boolean {
  if (text.length !== 1) return false
  const code = text.charCodeAt(0)
  if (code === 0x1b) return true
  return code < 0x20 && text !== '\r' && text !== '\n' && text !== '\t' && text !== '\b'
}

/**
 * The `terminal.input` messages for pasted images and any text pasted with
 * them, in order: each path as its own bracketed paste and a space, then
 * the text as a bracketed paste of its own.
 */
export function imagePasteInputs(
  paths: readonly string[],
  text: string,
): { kind: 'ready'; messages: string[] } | { kind: 'too_large'; message: string } {
  const messages: string[] = []
  for (const path of paths) {
    const paste = bracketedTerminalPaste(terminalPathText(path))
    if (paste?.kind === 'ready') messages.push(`${paste.text} `)
  }
  const textPaste = text ? bracketedTerminalPaste(text) : null
  if (textPaste?.kind === 'too_large') {
    return {
      kind: 'too_large',
      message: `the pasted text is ${formatTerminalBytes(textPaste.bytes)}, over the ${formatTerminalBytes(textPaste.limit)} terminal paste limit`,
    }
  }
  if (textPaste?.kind === 'ready') messages.push(textPaste.text)
  return { kind: 'ready', messages }
}

/**
 * Whether clipboard text only describes the pasted files: a file name, a
 * single URL, or a single path. Finder copies put the file name next to the
 * file, and Firefox's Copy Image adds the image URL; typing those after the
 * path would just add stray words to the prompt.
 */
export function isIncidentalClipboardText(text: string, files: readonly File[]) {
  const trimmed = text.trim()
  if (!trimmed) return true
  const lines = trimmed.split(/\r?\n/).map((line) => line.trim()).filter(Boolean)
  const names = new Set(files.map((file) => file.name).filter(Boolean))
  if (lines.every((line) => names.has(line))) return true
  if (lines.length !== 1 || /\s/.test(lines[0])) return false
  const token = lines[0]
  return /^[a-z][a-z0-9+.-]*:\S+$/i.test(token) || /^(~|\.{0,2})\/\S*$/.test(token)
}

export type ClipboardPastePlan =
  /** No images: an ordinary text paste (with a notice if images were skipped). */
  | { kind: 'text'; text: string; ignoredNotice: string | null }
  /** Upload the images, type their paths, then `text` (possibly empty). */
  | { kind: 'images'; images: File[]; text: string }
  /** Only unsupported images and no text. */
  | { kind: 'unsupported'; notice: string }
  | { kind: 'nothing' }

/**
 * What a clipboard paste should do.
 *
 * - Text that only names the pasted files (see
 *   {@link isIncidentalClipboardText}) is dropped.
 * - Rich copies from apps such as Excel, Numbers, Keynote or Word carry a
 *   PNG rendering of the copied content next to its text and HTML/RTF. The
 *   text is what the user meant, so it is pasted as before and the image is
 *   ignored with a notice.
 * - Otherwise images go first and the text follows.
 */
export function clipboardPastePlan(
  data: Pick<DataTransfer, 'items' | 'files' | 'types'> | null | undefined,
  text: string,
): ClipboardPastePlan {
  const { images, unsupported } = transferImages(data)
  const files = [...images]
  const incidental = isIncidentalClipboardText(text, files)
  if (images.length === 0) {
    if (unsupported.length === 0) return text ? { kind: 'text', text, ignoredNotice: null } : { kind: 'nothing' }
    if (!text) return { kind: 'unsupported', notice: unsupportedImageNotice(unsupported) }
    return {
      kind: 'text',
      text,
      ignoredNotice: `${unsupportedImageNotice(unsupported)}; only the text was pasted`,
    }
  }
  if (incidental) return { kind: 'images', images, text: '' }
  const types = data?.types ? Array.from(data.types) : []
  const rich = types.some((type) => type === 'text/html' || type === 'text/rtf')
  if (rich) {
    return {
      kind: 'text',
      text,
      ignoredNotice:
        images.length === 1
          ? 'Pasted the copied text; its image rendering was not uploaded'
          : 'Pasted the copied text; its image renderings were not uploaded',
    }
  }
  return { kind: 'images', images, text }
}

/** The notice for a drop: what was skipped, or why nothing was accepted. */
export function dropNotice(images: readonly File[], skipped: readonly File[]) {
  if (images.length === 0) {
    return 'Only PNG, JPEG, GIF or WebP images can be dropped into a terminal'
  }
  if (skipped.length === 0) return null
  const names = skipped.map((file) => file.name || file.type || 'file').join(', ')
  return `Skipped ${skipped.length === 1 ? 'a file that is' : `${skipped.length} files that are`} not a PNG, JPEG, GIF or WebP image: ${names}`
}

export function imagePasteNotice(images: readonly PastedTerminalImage[]) {
  if (images.length === 1) return `Image pasted → ${images[0].name}`
  return `${images.length} images pasted → ${images.map((image) => image.name).join(', ')}`
}

export function unsupportedImageNotice(types: readonly string[]) {
  const listed = [...new Set(types)].join(', ')
  return `Image not pasted: ${listed} is not supported; paste a PNG, JPEG, GIF or WebP image`
}

export type ImagePasteResult =
  | { kind: 'sent'; images: PastedTerminalImage[] }
  | { kind: 'error'; message: string }

export interface ImagePasteOptions {
  images: readonly File[]
  /** Text pasted with the images, sent after their paths. */
  text: string
  upload: (image: File) => Promise<PastedTerminalImage>
  /** Whether the socket the paste started on is still open. */
  attached: () => boolean
  /** Send one `terminal.input`; false when the socket is not open. */
  send: (text: string) => boolean
  /** Wait between messages; defaults to {@link IMAGE_PASTE_GAP_MS}. */
  pause?: () => Promise<void>
}

const defaultPause = () =>
  new Promise<void>((resolve) => {
    setTimeout(resolve, IMAGE_PASTE_GAP_MS)
  })

/**
 * Upload each image, then type the paths, each as its own message with a
 * short pause between them, and the text last. Nothing is typed unless the
 * same terminal is still attached once every upload finished, and typing
 * stops if it detaches between messages.
 */
export async function pasteTerminalImages(
  options: ImagePasteOptions,
): Promise<ImagePasteResult> {
  if (!options.attached()) {
    return { kind: 'error', message: 'Image not pasted: the terminal is not connected' }
  }
  // Refuse oversized text before uploading anything for it.
  const precheck = imagePasteInputs([], options.text)
  if (precheck.kind === 'too_large') {
    return { kind: 'error', message: `Paste not sent: ${precheck.message}` }
  }
  const stored: PastedTerminalImage[] = []
  for (const image of options.images) {
    try {
      stored.push(await options.upload(image))
    } catch (caught) {
      return { kind: 'error', message: `Image not pasted: ${uploadFailure(caught)}` }
    }
  }
  const input = imagePasteInputs(
    stored.map((image) => image.path),
    options.text,
  )
  if (input.kind === 'too_large') {
    return { kind: 'error', message: `Paste not sent: ${input.message}` }
  }
  const pause = options.pause ?? defaultPause
  for (const [index, message] of input.messages.entries()) {
    if (index > 0) await pause()
    if (!options.attached() || !options.send(message)) {
      return {
        kind: 'error',
        message:
          index === 0
            ? 'Image not pasted: the terminal disconnected during the upload'
            : 'Paste incomplete: the terminal disconnected while the image paths were typed',
      }
    }
  }
  return { kind: 'sent', images: stored }
}

function uploadFailure(caught: unknown) {
  if (caught instanceof YardApiError) return caught.message
  if (caught instanceof TypeError) return 'Yard is offline or unreachable'
  if (caught instanceof DOMException && caught.name === 'AbortError') {
    return 'the upload was cancelled'
  }
  return 'the upload failed'
}
