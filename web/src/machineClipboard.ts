export async function copyMachineCommand(value: string) {
  const writeText = navigator.clipboard?.writeText
  if (typeof writeText !== 'function') {
    throw Error('Clipboard is unavailable.')
  }
  await writeText.call(navigator.clipboard, value)
}
