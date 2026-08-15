import type { ITheme } from '@xterm/xterm'

export type YardTheme = 'dark' | 'light'

export const THEME_STORAGE_KEY = 'yard:theme'
export const THEME_CHANGE_EVENT = 'yard:theme-change'

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
