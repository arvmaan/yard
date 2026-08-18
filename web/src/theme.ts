import type { ITheme } from '@xterm/xterm'

export type YardTheme = 'dark' | 'light'

export const THEME_STORAGE_KEY = 'yard:theme'
export const THEME_CHANGE_EVENT = 'yard:theme-change'

/**
 * Named, hardcoded terminal color schemes, independent of the app-wide
 * light/dark theme. Selecting one of these changes only the xterm.js
 * palette inside the terminal — it never touches the rest of the app's
 * UI colors. "auto" preserves the historical behavior of deriving the
 * terminal palette from the app's own light/dark theme tokens.
 */
export type TerminalPaletteId =
  | 'auto'
  | 'nord'
  | 'dracula'
  | 'solarized-dark'
  | 'solarized-light'
  | 'gruvbox-dark'

export const TERMINAL_PALETTE_STORAGE_KEY = 'yard:terminal-palette'
export const TERMINAL_PALETTE_CHANGE_EVENT = 'yard:terminal-palette-change'

export const TERMINAL_PALETTE_OPTIONS: {
  id: TerminalPaletteId
  label: string
}[] = [
  { id: 'auto', label: 'Auto (match app theme)' },
  { id: 'nord', label: 'Nord' },
  { id: 'dracula', label: 'Dracula' },
  { id: 'solarized-dark', label: 'Solarized Dark' },
  { id: 'solarized-light', label: 'Solarized Light' },
  { id: 'gruvbox-dark', label: 'Gruvbox Dark' },
]

const TERMINAL_PALETTE_IDS = new Set<string>(
  TERMINAL_PALETTE_OPTIONS.map((option) => option.id),
)

// Nord — https://www.nordtheme.com/docs/colors-and-palettes
// nord0/1/2/3 (polar night), nord4/5/6 (snow storm), nord7-10 (frost),
// nord11-15 (aurora).
const NORD: ITheme = {
  background: '#2e3440',
  black: '#3b4252',
  blue: '#81a1c1',
  brightBlack: '#4c566a',
  brightBlue: '#81a1c1',
  brightCyan: '#8fbcbb',
  brightGreen: '#a3be8c',
  brightMagenta: '#b48ead',
  brightRed: '#bf616a',
  brightWhite: '#eceff4',
  brightYellow: '#ebcb8b',
  cursor: '#d8dee9',
  cursorAccent: '#2e3440',
  cyan: '#88c0d0',
  foreground: '#d8dee9',
  green: '#a3be8c',
  magenta: '#b48ead',
  red: '#bf616a',
  selectionBackground: '#434c5e',
  white: '#e5e9f0',
  yellow: '#ebcb8b',
}

// Dracula — https://draculatheme.com/contribute (official spec palette).
const DRACULA: ITheme = {
  background: '#282a36',
  black: '#21222c',
  blue: '#bd93f9',
  brightBlack: '#6272a4',
  brightBlue: '#d6acff',
  brightCyan: '#a4ffff',
  brightGreen: '#69ff94',
  brightMagenta: '#ff92df',
  brightRed: '#ff6e6e',
  brightWhite: '#ffffff',
  brightYellow: '#ffffa5',
  cursor: '#f8f8f2',
  cursorAccent: '#282a36',
  cyan: '#8be9fd',
  foreground: '#f8f8f2',
  green: '#50fa7b',
  magenta: '#ff79c6',
  red: '#ff5555',
  selectionBackground: '#44475a',
  white: '#f8f8f2',
  yellow: '#f1fa8c',
}

// Solarized (dark) — https://ethanschoonover.com/solarized/ precision
// color table, mapped to the standard 16-color ANSI assignment.
const SOLARIZED_DARK: ITheme = {
  background: '#002b36',
  black: '#073642',
  blue: '#268bd2',
  brightBlack: '#002b36',
  brightBlue: '#839496',
  brightCyan: '#93a1a1',
  brightGreen: '#586e75',
  brightMagenta: '#6c71c4',
  brightRed: '#cb4b16',
  brightWhite: '#fdf6e3',
  brightYellow: '#657b83',
  cursor: '#839496',
  cursorAccent: '#002b36',
  cyan: '#2aa198',
  foreground: '#839496',
  green: '#859900',
  magenta: '#d33682',
  red: '#dc322f',
  selectionBackground: '#073642',
  white: '#eee8d5',
  yellow: '#b58900',
}

// Solarized (light) — same accent hues as Solarized Dark, with the
// background/content tones inverted per the official spec.
const SOLARIZED_LIGHT: ITheme = {
  background: '#fdf6e3',
  black: '#eee8d5',
  blue: '#268bd2',
  brightBlack: '#fdf6e3',
  brightBlue: '#657b83',
  brightCyan: '#586e75',
  brightGreen: '#93a1a1',
  brightMagenta: '#6c71c4',
  brightRed: '#cb4b16',
  brightWhite: '#002b36',
  brightYellow: '#839496',
  cursor: '#657b83',
  cursorAccent: '#fdf6e3',
  cyan: '#2aa198',
  foreground: '#657b83',
  green: '#859900',
  magenta: '#d33682',
  red: '#dc322f',
  selectionBackground: '#eee8d5',
  white: '#073642',
  yellow: '#b58900',
}

// Gruvbox (dark, medium contrast) — https://github.com/morhetz/gruvbox
// palette-generation.md standard 16-color assignment.
const GRUVBOX_DARK: ITheme = {
  background: '#282828',
  black: '#282828',
  blue: '#458588',
  brightBlack: '#928374',
  brightBlue: '#83a598',
  brightCyan: '#8ec07c',
  brightGreen: '#b8bb26',
  brightMagenta: '#d3869b',
  brightRed: '#fb4934',
  brightWhite: '#ebdbb2',
  brightYellow: '#fabd2f',
  cursor: '#ebdbb2',
  cursorAccent: '#282828',
  cyan: '#689d6a',
  foreground: '#ebdbb2',
  green: '#98971a',
  magenta: '#b16286',
  red: '#cc241d',
  selectionBackground: '#504945',
  white: '#7c6f64',
  yellow: '#d79921',
}

const STATIC_TERMINAL_PALETTES: Partial<Record<TerminalPaletteId, ITheme>> = {
  dracula: DRACULA,
  'gruvbox-dark': GRUVBOX_DARK,
  nord: NORD,
  'solarized-dark': SOLARIZED_DARK,
  'solarized-light': SOLARIZED_LIGHT,
}

export function readTerminalPalette(): TerminalPaletteId {
  const stored = window.localStorage.getItem(TERMINAL_PALETTE_STORAGE_KEY)
  if (stored && TERMINAL_PALETTE_IDS.has(stored)) {
    return stored as TerminalPaletteId
  }
  return 'auto'
}

export function applyTerminalPalette(palette: TerminalPaletteId) {
  window.localStorage.setItem(TERMINAL_PALETTE_STORAGE_KEY, palette)
  window.dispatchEvent(
    new CustomEvent<TerminalPaletteId>(TERMINAL_PALETTE_CHANGE_EVENT, {
      detail: palette,
    }),
  )
}

/**
 * Resolves the effective xterm.js theme for a terminal, given the app's
 * current light/dark theme and the user's terminal palette preference.
 * "auto" (the default) keeps deriving colors live from the app theme, as
 * it always has; any named palette overrides it with a fixed ITheme that
 * does not change when the app-wide light/dark toggle changes.
 */
export function resolveTerminalTheme(
  appTheme: YardTheme,
  palette: TerminalPaletteId,
): ITheme {
  if (palette === 'auto') return terminalTheme(appTheme)
  return STATIC_TERMINAL_PALETTES[palette] ?? terminalTheme(appTheme)
}

/**
 * CSS custom property overrides for the terminal's own chrome (viewport
 * background, border, status bar) so it matches a named palette. Returns
 * undefined for "auto", leaving the existing app-theme-driven CSS
 * variables (set on `:root[data-theme]`) untouched.
 */
export function terminalChromeVariables(
  palette: TerminalPaletteId,
): Record<string, string> | undefined {
  if (palette === 'auto') return undefined
  const swatch = STATIC_TERMINAL_PALETTES[palette]
  if (!swatch) return undefined
  const chromeForeground =
    palette === 'solarized-light' ? swatch.brightWhite : swatch.foreground
  return {
    '--terminal-bg': swatch.background as string,
    '--terminal-ink': swatch.foreground as string,
    '--terminal-line': chromeForeground as string,
    '--terminal-muted': chromeForeground as string,
    '--terminal-selection': swatch.selectionBackground as string,
  }
}

export function readTheme(): YardTheme {
  const stored = window.localStorage.getItem(THEME_STORAGE_KEY)
  if (stored === 'dark' || stored === 'light') return stored
  return window.matchMedia('(prefers-color-scheme: dark)').matches
    ? 'dark'
    : 'light'
}

export function applyTheme(theme: YardTheme) {
  document.documentElement.dataset.theme = theme
  document.documentElement.style.colorScheme = theme
  window.localStorage.setItem(THEME_STORAGE_KEY, theme)
  window.dispatchEvent(
    new CustomEvent<YardTheme>(THEME_CHANGE_EVENT, { detail: theme }),
  )
}

function themeToken(name: string, fallback: string): string {
  const value = window
    .getComputedStyle(document.documentElement)
    .getPropertyValue(name)
    .trim()
  return value || fallback
}

export function terminalTheme(theme: YardTheme): ITheme {
  const light = theme === 'light'
  const background = themeToken(
    '--surface-muted',
    light ? '#edf1ef' : '#17201d',
  )
  const foreground = themeToken('--ink', light ? '#18201e' : '#e1ebe7')
  const muted = themeToken('--muted', light ? '#68736f' : '#9aa9a3')
  const teal = themeToken('--teal', light ? '#19766b' : '#65c7bb')
  const blue = themeToken('--blue', light ? '#3978b8' : '#79afe3')
  const yellow = themeToken('--yellow', light ? '#e7b32b' : '#f0c44e')
  const red = themeToken('--vermilion', light ? '#d94a37' : '#ff8d7d')

  return {
    background,
    black: light ? foreground : themeToken('--canvas', '#111715'),
    blue,
    brightBlack: muted,
    brightBlue: blue,
    brightCyan: teal,
    brightGreen: light ? '#3f7f52' : '#91d0a2',
    brightMagenta: light ? '#8a5e96' : '#d7a8df',
    brightRed: red,
    brightWhite: themeToken('--surface-raised', '#ffffff'),
    brightYellow: yellow,
    cursor: teal,
    cursorAccent: background,
    cyan: teal,
    foreground,
    green: light ? '#3f7f52' : '#72b783',
    magenta: light ? '#7c5588' : '#bf8dca',
    red,
    selectionBackground: themeToken(
      '--terminal-selection',
      light ? '#bfd7d1' : '#305c56',
    ),
    white: light ? '#dfe5e2' : foreground,
    yellow: light ? '#8c6711' : yellow,
  }
}
