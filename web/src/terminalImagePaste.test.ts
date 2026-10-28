import { describe, expect, it, vi } from 'vitest'
import { type PastedTerminalImage, terminalImageUploadPath, YardApiError } from './api'
import {
  clipboardPastePlan,
  dropNotice,
  IMAGE_PASTE_GAP_MS,
  imagePasteInputs,
  imagePasteNotice,
  isIncidentalClipboardText,
  isTerminalControlInput,
  pasteTerminalImages,
  terminalPathText,
  transferImages,
  unsupportedImageNotice,
} from './terminalImagePaste'

const START = '\u001b[200~'
const END = '\u001b[201~'

function file(name: string, type: string) {
  return new File([new Uint8Array([1, 2, 3])], name, { type })
}

function items(files: File[], extra: { kind: string }[] = [], types: string[] = []) {
  return {
    items: [
      ...extra.map((item) => ({ kind: item.kind, getAsFile: () => null })),
      ...files.map((entry) => ({ kind: 'file', getAsFile: () => entry })),
    ] as unknown as DataTransferItemList,
    files: [] as unknown as FileList,
    types: [...types, ...(files.length > 0 ? ['Files'] : [])] as readonly string[],
  }
}

function stored(name: string): PastedTerminalImage {
  return {
    path: `/data/yard/pastes/worker-1/${name}`,
    name,
    bytes: 3,
    content_type: 'image/png',
  }
}

describe('transferImages', () => {
  it('keeps supported images in order and lists other image types', () => {
    const png = file('a.png', 'image/png')
    const webp = file('b.webp', 'IMAGE/WEBP')
    const result = transferImages(
      items([png, file('c.tiff', 'image/tiff'), webp, file('d.txt', 'text/plain')], [
        { kind: 'string' },
      ]),
    )
    expect(result.images).toEqual([png, webp])
    expect(result.unsupported).toEqual(['image/tiff'])
    expect(result.skipped.map((entry) => entry.name)).toEqual(['c.tiff', 'd.txt'])
  })

  it('falls back to files for drops and handles a missing transfer', () => {
    const gif = file('a.gif', 'image/gif')
    const result = transferImages({
      items: [] as unknown as DataTransferItemList,
      files: [gif] as unknown as FileList,
    })
    expect(result.images).toEqual([gif])
    expect(transferImages(null)).toEqual({ images: [], unsupported: [], skipped: [] })
  })
})

describe('imagePasteInputs', () => {
  it('types each path as its own bracketed paste and a space, without Enter', () => {
    const input = imagePasteInputs(['/a/one.png', '/b/my shot.png'], '')
    expect(input).toEqual({
      kind: 'ready',
      messages: [`${START}/a/one.png${END} `, `${START}/b/my\\ shot.png${END} `],
    })
    expect(input.kind === 'ready' && input.messages.some((text) => text.includes('\r'))).toBe(
      false,
    )
  })

  it('sends pasted text after the paths, as a paste of its own', () => {
    expect(imagePasteInputs(['/a/one.png'], 'describe\nthis')).toEqual({
      kind: 'ready',
      messages: [`${START}/a/one.png${END} `, `${START}describe\rthis${END}`],
    })
  })

  it('refuses text over the terminal paste limit', () => {
    const input = imagePasteInputs(['/a/one.png'], 'y'.repeat(600 * 1024))
    expect(input.kind).toBe('too_large')
  })

  it('escapes spaces and backslashes in paths', () => {
    expect(terminalPathText('/a b\\c.png')).toBe('/a\\ b\\\\c.png')
  })
})

describe('clipboardPastePlan', () => {
  const png = file('Screenshot 2026-10-19.png', 'image/png')

  it('pastes plain text as before', () => {
    expect(clipboardPastePlan(items([], [{ kind: 'string' }], ['text/plain']), 'hello')).toEqual({
      kind: 'text',
      text: 'hello',
      ignoredNotice: null,
    })
    expect(clipboardPastePlan(items([]), '')).toEqual({ kind: 'nothing' })
  })

  it('prefers the text of a rich copy that also carries a PNG rendering', () => {
    // Excel, Numbers, Keynote and Word put text, HTML/RTF and a PNG on the clipboard.
    const cells = 'Region\tTotal\nWest\t42'
    const plan = clipboardPastePlan(
      items([file('image.png', 'image/png')], [], ['text/plain', 'text/html', 'text/rtf']),
      cells,
    )
    expect(plan).toEqual({
      kind: 'text',
      text: cells,
      ignoredNotice: 'Pasted the copied text; its image rendering was not uploaded',
    })
  })

  it('drops text that only names the pasted file (Finder copy, Copy Image)', () => {
    expect(clipboardPastePlan(items([png], [], ['text/plain']), png.name)).toEqual({
      kind: 'images',
      images: [png],
      text: '',
    })
    expect(
      clipboardPastePlan(
        items([png], [], ['text/plain', 'text/html']),
        'https://example.com/shot.png',
      ),
    ).toEqual({ kind: 'images', images: [png], text: '' })
  })

  it('sends the paths and then real text pasted with an image', () => {
    expect(clipboardPastePlan(items([png], [], ['text/plain']), 'what is wrong here?')).toEqual({
      kind: 'images',
      images: [png],
      text: 'what is wrong here?',
    })
  })

  it('explains an unsupported image, with or without text', () => {
    const tiff = file('icon.tiff', 'image/tiff')
    expect(clipboardPastePlan(items([tiff]), '')).toEqual({
      kind: 'unsupported',
      notice: unsupportedImageNotice(['image/tiff']),
    })
    // Safari's Finder copy: a TIFF icon plus the file name as text.
    expect(clipboardPastePlan(items([tiff], [], ['text/plain']), 'report.pdf')).toEqual({
      kind: 'text',
      text: 'report.pdf',
      ignoredNotice: `${unsupportedImageNotice(['image/tiff'])}; only the text was pasted`,
    })
  })

  it('recognises incidental text', () => {
    const files = [png]
    expect(isIncidentalClipboardText('', files)).toBe(true)
    expect(isIncidentalClipboardText(`  ${png.name}\n`, files)).toBe(true)
    expect(isIncidentalClipboardText('file:///Users/me/shot.png', files)).toBe(true)
    expect(isIncidentalClipboardText('/Users/me/shot.png', files)).toBe(true)
    expect(isIncidentalClipboardText('fix this', files)).toBe(false)
    expect(isIncidentalClipboardText('see https://example.com', files)).toBe(false)
  })
})

describe('dropNotice', () => {
  it('explains refused and skipped drops', () => {
    const png = file('a.png', 'image/png')
    const pdf = file('b.pdf', 'application/pdf')
    expect(dropNotice([], [pdf])).toBe(
      'Only PNG, JPEG, GIF or WebP images can be dropped into a terminal',
    )
    expect(dropNotice([png], [])).toBeNull()
    expect(dropNotice([png], [pdf])).toBe(
      'Skipped a file that is not a PNG, JPEG, GIF or WebP image: b.pdf',
    )
  })
})

describe('notices', () => {
  it('names the stored images and the unsupported types', () => {
    expect(imagePasteNotice([stored('x.png')])).toBe('Image pasted → x.png')
    expect(imagePasteNotice([stored('x.png'), stored('y.png')])).toBe(
      '2 images pasted → x.png, y.png',
    )
    expect(unsupportedImageNotice(['image/tiff', 'image/tiff'])).toBe(
      'Image not pasted: image/tiff is not supported; paste a PNG, JPEG, GIF or WebP image',
    )
  })
})

describe('isTerminalControlInput', () => {
  it('lets interrupts and Esc past a pending image paste', () => {
    for (const key of ['\u0003', '\u001b', '\u0004', '\u001a', '\u001c']) {
      expect(isTerminalControlInput(key)).toBe(true)
    }
  })

  it('keeps editing keys, text and escape sequences in order', () => {
    for (const key of ['\r', '\n', '\t', '\b', '\u007f', 'a', 'ab', '\u001b[A', '']) {
      expect(isTerminalControlInput(key)).toBe(false)
    }
  })
})

describe('terminalImageUploadPath', () => {
  it('mirrors each terminal WebSocket route', () => {
    expect(
      terminalImageUploadPath({ kind: 'assignment', projectId: 'p 1', assignmentId: 'a/1' }),
    ).toBe('/api/v1/projects/p%201/assignments/a%2F1/terminal/images')
    expect(terminalImageUploadPath({ kind: 'orchestrator', projectId: 'p' })).toBe(
      '/api/v1/projects/p/orchestrator/terminal/images',
    )
    expect(terminalImageUploadPath({ kind: 'yard-orchestrator' })).toBe(
      '/api/v1/yard/orchestrator/terminal/images',
    )
    expect(terminalImageUploadPath({ kind: 'coordination-node', nodeId: 'n' })).toBe(
      '/api/v1/coordination-nodes/n/terminal/images',
    )
  })
})

describe('pasteTerminalImages', () => {
  const png = file('a.png', 'image/png')

  it('uploads every image, then sends each path and the text as separate pastes', async () => {
    const events: string[] = []
    const send = vi.fn((text: string) => {
      events.push(`send ${text}`)
      return true
    })
    const upload = vi
      .fn()
      .mockResolvedValueOnce(stored('one.png'))
      .mockResolvedValueOnce(stored('two.png'))
    const result = await pasteTerminalImages({
      images: [png, png],
      text: 'what is this',
      upload,
      attached: () => true,
      send,
      pause: async () => {
        events.push('pause')
      },
    })
    expect(result).toEqual({ kind: 'sent', images: [stored('one.png'), stored('two.png')] })
    expect(upload).toHaveBeenCalledTimes(2)
    expect(events).toEqual([
      `send ${START}/data/yard/pastes/worker-1/one.png${END} `,
      'pause',
      `send ${START}/data/yard/pastes/worker-1/two.png${END} `,
      'pause',
      `send ${START}what is this${END}`,
    ])
  })

  it('waits the paste gap between messages by default', async () => {
    vi.useFakeTimers()
    try {
      const send = vi.fn(() => true)
      const pending = pasteTerminalImages({
        images: [png],
        text: 'and text',
        upload: async () => stored('one.png'),
        attached: () => true,
        send,
      })
      await vi.advanceTimersByTimeAsync(0)
      expect(send).toHaveBeenCalledTimes(1)
      await vi.advanceTimersByTimeAsync(IMAGE_PASTE_GAP_MS - 1)
      expect(send).toHaveBeenCalledTimes(1)
      await vi.advanceTimersByTimeAsync(1)
      expect(send).toHaveBeenCalledTimes(2)
      expect((await pending).kind).toBe('sent')
    } finally {
      vi.useRealTimers()
    }
  })

  it('stops typing if the terminal detaches between pastes', async () => {
    let attached = true
    const send = vi.fn(() => true)
    const result = await pasteTerminalImages({
      images: [png, png],
      text: '',
      upload: async () => stored('one.png'),
      attached: () => attached,
      send,
      pause: async () => {
        attached = false
      },
    })
    expect(result).toEqual({
      kind: 'error',
      message: 'Paste incomplete: the terminal disconnected while the image paths were typed',
    })
    expect(send).toHaveBeenCalledOnce()
  })

  it('uploads nothing for a terminal that is not attached', async () => {
    const upload = vi.fn()
    const send = vi.fn(() => true)
    const result = await pasteTerminalImages({
      images: [png],
      text: '',
      upload,
      attached: () => false,
      send,
    })
    expect(result).toEqual({
      kind: 'error',
      message: 'Image not pasted: the terminal is not connected',
    })
    expect(upload).not.toHaveBeenCalled()
    expect(send).not.toHaveBeenCalled()
  })

  it('types nothing when the terminal detached during the upload', async () => {
    let attached = true
    const send = vi.fn(() => true)
    const result = await pasteTerminalImages({
      images: [png],
      text: '',
      upload: async () => {
        attached = false
        return stored('one.png')
      },
      attached: () => attached,
      send,
    })
    expect(result).toEqual({
      kind: 'error',
      message: 'Image not pasted: the terminal disconnected during the upload',
    })
    expect(send).not.toHaveBeenCalled()
  })

  it('refuses oversized text before uploading', async () => {
    const upload = vi.fn()
    const result = await pasteTerminalImages({
      images: [png],
      text: 'y'.repeat(600 * 1024),
      upload,
      attached: () => true,
      send: () => true,
    })
    expect(result.kind).toBe('error')
    expect(result.kind === 'error' && result.message).toMatch(/^Paste not sent: the pasted text/)
    expect(upload).not.toHaveBeenCalled()
  })

  it.each([
    [new YardApiError('image_too_large', 'the image is 11 bytes; the limit is 10 bytes'), 'the image is 11 bytes; the limit is 10 bytes'],
    [new TypeError('Failed to fetch'), 'Yard is offline or unreachable'],
    [new DOMException('aborted', 'AbortError'), 'the upload was cancelled'],
    [new Error('boom'), 'the upload failed'],
  ])('explains an upload failure (%s) and types nothing', async (error, message) => {
    const send = vi.fn(() => true)
    const result = await pasteTerminalImages({
      images: [png, png],
      text: '',
      upload: async () => {
        throw error
      },
      attached: () => true,
      send,
    })
    expect(result).toEqual({ kind: 'error', message: `Image not pasted: ${message}` })
    expect(send).not.toHaveBeenCalled()
  })
})
