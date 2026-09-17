import type { ITheme } from '@xterm/xterm'

export type ThemeId = string

export interface ThemeTokens {
  accentPrimary: string
  accentPrimarySoft: string
  accentSecondary: string
  borderStrong: string
  borderSubtle: string
  insetHighlight: string
  labelShadow: string
  mapDepthFace: string
  mapDepthShadow: string
  mapGridMajor: string
  mapGridMinor: string
  mapMask: string
  overlay: string
  shadow: string
  shadowStrong: string
  statusDanger: string
  statusDangerSoft: string
  statusInfo: string
  statusInfoSoft: string
  statusSuccess: string
  statusWarningForeground: string
  statusWarning: string
  statusWarningSoft: string
  surfaceCanvas: string
  surfaceMuted: string
  surfacePanel: string
  surfaceRaised: string
  surfaceSubtle: string
  textMuted: string
  textOnAccent: string
  textPrimary: string
}

export interface ThemeDefinition {
  colorScheme: 'dark' | 'light'
  id: ThemeId
  label: string
  terminal: ITheme
  tokens: ThemeTokens
}

export const THEME_STORAGE_KEY = 'yard:theme'
export const THEME_CHANGE_EVENT = 'yard:theme-change'

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
  selectionForeground: '#eee8d5',
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
  foreground: '#586e75',
  green: '#859900',
  magenta: '#d33682',
  red: '#dc322f',
  selectionBackground: '#eee8d5',
  selectionForeground: '#073642',
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

const THEMES: readonly ThemeDefinition[] = [
  {
    id: 'light',
    label: 'Yard Light',
    colorScheme: 'light',
    tokens: {
      accentPrimary: '#19766b',
      accentPrimarySoft: '#d9ece8',
      accentSecondary: '#8a5e96',
      borderStrong: '#9eada6',
      borderSubtle: '#cbd3cf',
      insetHighlight: 'rgba(255, 255, 255, 0.52)',
      labelShadow: 'rgba(255, 255, 255, 0.94)',
      mapDepthFace: '#aebbb5',
      mapDepthShadow: 'rgba(23, 32, 30, 0.2)',
      mapGridMajor: 'rgba(23, 32, 30, 0.11)',
      mapGridMinor: 'rgba(23, 32, 30, 0.055)',
      mapMask: 'rgba(229, 233, 231, 0.76)',
      overlay: 'rgba(23, 32, 30, 0.46)',
      shadow: '0 10px 28px rgba(23, 32, 30, 0.12)',
      shadowStrong: '0 24px 72px rgba(6, 10, 9, 0.28)',
      statusDanger: '#d94a37',
      statusDangerSoft: '#fce3df',
      statusInfo: '#3978b8',
      statusInfoSoft: '#e4eef8',
      statusSuccess: '#3f7f52',
      statusWarningForeground: '#6b4d00',
      statusWarning: '#e7b32b',
      statusWarningSoft: '#fff4cf',
      surfaceCanvas: '#dce2df',
      surfaceMuted: '#edf1ef',
      surfacePanel: '#f7f9f8',
      surfaceRaised: '#ffffff',
      surfaceSubtle: '#e5e9e7',
      textMuted: '#646f6b',
      textOnAccent: '#17201e',
      textPrimary: '#18201e',
    },
    terminal: {
      background: '#edf1ef',
      black: '#18201e',
      blue: '#3978b8',
      brightBlack: '#646f6b',
      brightBlue: '#3978b8',
      brightCyan: '#19766b',
      brightGreen: '#3f7f52',
      brightMagenta: '#8a5e96',
      brightRed: '#d94a37',
      brightWhite: '#ffffff',
      brightYellow: '#e7b32b',
      cursor: '#19766b',
      cursorAccent: '#edf1ef',
      cyan: '#19766b',
      foreground: '#18201e',
      green: '#3f7f52',
      magenta: '#7c5588',
      red: '#d94a37',
      selectionBackground: '#bfd7d1',
      white: '#dfe5e2',
      yellow: '#8c6711',
    },
  },
  {
    id: 'dark',
    label: 'Yard Dark',
    colorScheme: 'dark',
    tokens: {
      accentPrimary: '#65c7bb',
      accentPrimarySoft: '#203f3a',
      accentSecondary: '#d7a8df',
      borderStrong: '#56665f',
      borderSubtle: '#34413c',
      insetHighlight: 'rgba(255, 255, 255, 0.1)',
      labelShadow: 'rgba(17, 23, 21, 0.96)',
      mapDepthFace: '#26332e',
      mapDepthShadow: 'rgba(0, 0, 0, 0.42)',
      mapGridMajor: 'rgba(225, 235, 231, 0.1)',
      mapGridMinor: 'rgba(225, 235, 231, 0.05)',
      mapMask: 'rgba(17, 23, 21, 0.76)',
      overlay: 'rgba(4, 8, 7, 0.7)',
      shadow: '0 12px 32px rgba(0, 0, 0, 0.28)',
      shadowStrong: '0 24px 72px rgba(0, 0, 0, 0.48)',
      statusDanger: '#ff8d7d',
      statusDangerSoft: '#4a2925',
      statusInfo: '#79afe3',
      statusInfoSoft: '#243b50',
      statusSuccess: '#91d0a2',
      statusWarningForeground: '#f0c44e',
      statusWarning: '#f0c44e',
      statusWarningSoft: '#443a1d',
      surfaceCanvas: '#111715',
      surfaceMuted: '#17201d',
      surfacePanel: '#1b2421',
      surfaceRaised: '#202b27',
      surfaceSubtle: '#151c19',
      textMuted: '#9aa9a3',
      textOnAccent: '#111715',
      textPrimary: '#e1ebe7',
    },
    terminal: {
      background: '#17201d',
      black: '#111715',
      blue: '#79afe3',
      brightBlack: '#9aa9a3',
      brightBlue: '#79afe3',
      brightCyan: '#65c7bb',
      brightGreen: '#91d0a2',
      brightMagenta: '#d7a8df',
      brightRed: '#ff8d7d',
      brightWhite: '#ffffff',
      brightYellow: '#f0c44e',
      cursor: '#65c7bb',
      cursorAccent: '#17201d',
      cyan: '#65c7bb',
      foreground: '#e1ebe7',
      green: '#72b783',
      magenta: '#bf8dca',
      red: '#ff8d7d',
      selectionBackground: '#305c56',
      white: '#e1ebe7',
      yellow: '#f0c44e',
    },
  },
  {
    id: 'nord',
    label: 'Nord',
    colorScheme: 'dark',
    tokens: {
      accentPrimary: '#88c0d0',
      accentPrimarySoft: '#334854',
      accentSecondary: '#b48ead',
      borderStrong: '#66758c',
      borderSubtle: '#4c566a',
      insetHighlight: 'rgba(236, 239, 244, 0.1)',
      labelShadow: 'rgba(36, 41, 51, 0.96)',
      mapDepthFace: '#3b4252',
      mapDepthShadow: 'rgba(0, 0, 0, 0.42)',
      mapGridMajor: 'rgba(216, 222, 233, 0.11)',
      mapGridMinor: 'rgba(216, 222, 233, 0.055)',
      mapMask: 'rgba(36, 41, 51, 0.78)',
      overlay: 'rgba(18, 21, 27, 0.72)',
      shadow: '0 12px 32px rgba(0, 0, 0, 0.3)',
      shadowStrong: '0 24px 72px rgba(0, 0, 0, 0.5)',
      statusDanger: '#bf616a',
      statusDangerSoft: '#493238',
      statusInfo: '#81a1c1',
      statusInfoSoft: '#344453',
      statusSuccess: '#a3be8c',
      statusWarningForeground: '#ebcb8b',
      statusWarning: '#ebcb8b',
      statusWarningSoft: '#4a4331',
      surfaceCanvas: '#242933',
      surfaceMuted: '#2e3440',
      surfacePanel: '#2b303b',
      surfaceRaised: '#3b4252',
      surfaceSubtle: '#272d38',
      textMuted: '#b8c0cc',
      textOnAccent: '#242933',
      textPrimary: '#eceff4',
    },
    terminal: NORD,
  },
  {
    id: 'dracula',
    label: 'Dracula',
    colorScheme: 'dark',
    tokens: {
      accentPrimary: '#8be9fd',
      accentPrimarySoft: '#29454d',
      accentSecondary: '#ff79c6',
      borderStrong: '#6272a4',
      borderSubtle: '#44475a',
      insetHighlight: 'rgba(248, 248, 242, 0.1)',
      labelShadow: 'rgba(32, 33, 43, 0.96)',
      mapDepthFace: '#44475a',
      mapDepthShadow: 'rgba(0, 0, 0, 0.44)',
      mapGridMajor: 'rgba(248, 248, 242, 0.1)',
      mapGridMinor: 'rgba(248, 248, 242, 0.05)',
      mapMask: 'rgba(32, 33, 43, 0.78)',
      overlay: 'rgba(14, 14, 20, 0.74)',
      shadow: '0 12px 32px rgba(0, 0, 0, 0.3)',
      shadowStrong: '0 24px 72px rgba(0, 0, 0, 0.52)',
      statusDanger: '#ff6e6e',
      statusDangerSoft: '#512f3a',
      statusInfo: '#bd93f9',
      statusInfoSoft: '#3d3455',
      statusSuccess: '#69ff94',
      statusWarningForeground: '#f1fa8c',
      statusWarning: '#f1fa8c',
      statusWarningSoft: '#4d4c31',
      surfaceCanvas: '#20212b',
      surfaceMuted: '#282a36',
      surfacePanel: '#282a36',
      surfaceRaised: '#343746',
      surfaceSubtle: '#242632',
      textMuted: '#b8b9c8',
      textOnAccent: '#20212b',
      textPrimary: '#f8f8f2',
    },
    terminal: DRACULA,
  },
  {
    id: 'solarized-dark',
    label: 'Solarized Dark',
    colorScheme: 'dark',
    tokens: {
      accentPrimary: '#2aa198',
      accentPrimarySoft: '#073f43',
      accentSecondary: '#d33682',
      borderStrong: '#657b83',
      borderSubtle: '#1f4c57',
      insetHighlight: 'rgba(238, 232, 213, 0.09)',
      labelShadow: 'rgba(0, 33, 42, 0.96)',
      mapDepthFace: '#073642',
      mapDepthShadow: 'rgba(0, 0, 0, 0.44)',
      mapGridMajor: 'rgba(147, 161, 161, 0.12)',
      mapGridMinor: 'rgba(147, 161, 161, 0.06)',
      mapMask: 'rgba(0, 33, 42, 0.8)',
      overlay: 'rgba(0, 20, 25, 0.74)',
      shadow: '0 12px 32px rgba(0, 0, 0, 0.3)',
      shadowStrong: '0 24px 72px rgba(0, 0, 0, 0.52)',
      statusDanger: '#dc322f',
      statusDangerSoft: '#482c2d',
      statusInfo: '#268bd2',
      statusInfoSoft: '#123b55',
      statusSuccess: '#859900',
      statusWarningForeground: '#f2c95c',
      statusWarning: '#b58900',
      statusWarningSoft: '#493d1c',
      surfaceCanvas: '#001f27',
      surfaceMuted: '#002b36',
      surfacePanel: '#052f38',
      surfaceRaised: '#073642',
      surfaceSubtle: '#002730',
      textMuted: '#93a1a1',
      textOnAccent: '#00212a',
      textPrimary: '#eee8d5',
    },
    terminal: SOLARIZED_DARK,
  },
  {
    id: 'solarized-light',
    label: 'Solarized Light',
    colorScheme: 'light',
    tokens: {
      accentPrimary: '#2a8179',
      accentPrimarySoft: '#d7ebe6',
      accentSecondary: '#b82f75',
      borderStrong: '#7f9092',
      borderSubtle: '#c4bba4',
      insetHighlight: 'rgba(255, 255, 255, 0.56)',
      labelShadow: 'rgba(253, 246, 227, 0.96)',
      mapDepthFace: '#c9c1ad',
      mapDepthShadow: 'rgba(7, 54, 66, 0.2)',
      mapGridMajor: 'rgba(7, 54, 66, 0.12)',
      mapGridMinor: 'rgba(7, 54, 66, 0.06)',
      mapMask: 'rgba(238, 232, 213, 0.78)',
      overlay: 'rgba(7, 54, 66, 0.46)',
      shadow: '0 10px 28px rgba(7, 54, 66, 0.12)',
      shadowStrong: '0 24px 72px rgba(0, 43, 54, 0.28)',
      statusDanger: '#c73735',
      statusDangerSoft: '#f4ded6',
      statusInfo: '#2677b3',
      statusInfoSoft: '#dbe8ef',
      statusSuccess: '#657b12',
      statusWarningForeground: '#715100',
      statusWarning: '#9b7200',
      statusWarningSoft: '#f1e6bd',
      surfaceCanvas: '#eee8d5',
      surfaceMuted: '#f4edda',
      surfacePanel: '#fdf6e3',
      surfaceRaised: '#fffaf0',
      surfaceSubtle: '#f5efdf',
      textMuted: '#586e75',
      textOnAccent: '#fdf6e3',
      textPrimary: '#073642',
    },
    terminal: SOLARIZED_LIGHT,
  },
  {
    id: 'gruvbox-dark',
    label: 'Gruvbox Dark',
    colorScheme: 'dark',
    tokens: {
      accentPrimary: '#8ec07c',
      accentPrimarySoft: '#39483a',
      accentSecondary: '#d3869b',
      borderStrong: '#7c6f64',
      borderSubtle: '#504945',
      insetHighlight: 'rgba(235, 219, 178, 0.09)',
      labelShadow: 'rgba(29, 32, 33, 0.96)',
      mapDepthFace: '#3c3836',
      mapDepthShadow: 'rgba(0, 0, 0, 0.44)',
      mapGridMajor: 'rgba(235, 219, 178, 0.11)',
      mapGridMinor: 'rgba(235, 219, 178, 0.055)',
      mapMask: 'rgba(29, 32, 33, 0.79)',
      overlay: 'rgba(16, 17, 18, 0.74)',
      shadow: '0 12px 32px rgba(0, 0, 0, 0.3)',
      shadowStrong: '0 24px 72px rgba(0, 0, 0, 0.52)',
      statusDanger: '#fb4934',
      statusDangerSoft: '#512f28',
      statusInfo: '#83a598',
      statusInfoSoft: '#34433f',
      statusSuccess: '#b8bb26',
      statusWarningForeground: '#fabd2f',
      statusWarning: '#fabd2f',
      statusWarningSoft: '#50401f',
      surfaceCanvas: '#1d2021',
      surfaceMuted: '#282828',
      surfacePanel: '#282828',
      surfaceRaised: '#3c3836',
      surfaceSubtle: '#242424',
      textMuted: '#bdae93',
      textOnAccent: '#1d2021',
      textPrimary: '#ebdbb2',
    },
    terminal: GRUVBOX_DARK,
  },
]

export const THEME_REGISTRY = new Map(
  THEMES.map((theme) => [theme.id, theme]),
)

export const THEME_OPTIONS = THEMES.map(({ id, label }) => ({ id, label }))

const CSS_VARIABLES: Record<keyof ThemeTokens, string> = {
  accentPrimary: '--accent-primary',
  accentPrimarySoft: '--accent-primary-soft',
  accentSecondary: '--accent-secondary',
  borderStrong: '--border-strong',
  borderSubtle: '--border-subtle',
  insetHighlight: '--inset-highlight',
  labelShadow: '--label-shadow',
  mapDepthFace: '--map-depth-face',
  mapDepthShadow: '--map-depth-shadow',
  mapGridMajor: '--map-grid-major',
  mapGridMinor: '--map-grid-minor',
  mapMask: '--map-mask',
  overlay: '--overlay',
  shadow: '--shadow',
  shadowStrong: '--shadow-strong',
  statusDanger: '--status-danger',
  statusDangerSoft: '--status-danger-soft',
  statusInfo: '--status-info',
  statusInfoSoft: '--status-info-soft',
  statusSuccess: '--status-success',
  statusWarningForeground: '--status-warning-foreground',
  statusWarning: '--status-warning',
  statusWarningSoft: '--status-warning-soft',
  surfaceCanvas: '--surface-canvas',
  surfaceMuted: '--surface-muted',
  surfacePanel: '--surface-panel',
  surfaceRaised: '--surface-raised',
  surfaceSubtle: '--surface-subtle',
  textMuted: '--text-muted',
  textOnAccent: '--text-on-accent',
  textPrimary: '--text-primary',
}

export function themeDefinition(id: ThemeId): ThemeDefinition {
  return THEME_REGISTRY.get(id) ?? THEME_REGISTRY.get('light')!
}

export function readTheme(environment: {
  readonly localStorage?: Pick<Storage, 'getItem'>
  readonly matchMedia?: (
    query: string,
  ) => Pick<MediaQueryList, 'matches'>
} = window): ThemeId {
  try {
    const stored = environment.localStorage?.getItem(THEME_STORAGE_KEY)
    if (stored && THEME_REGISTRY.has(stored)) return stored
  } catch {
    // Browser storage may be unavailable; fall through to system preference.
  }
  try {
    return environment.matchMedia?.('(prefers-color-scheme: dark)').matches
      ? 'dark'
      : 'light'
  } catch {
    // Browser preference access can also be unavailable.
  }
  return 'light'
}

export function applyTheme(id: ThemeId) {
  const theme = themeDefinition(id)
  const root = document.documentElement
  root.dataset.theme = theme.id
  root.dataset.themeScheme = theme.colorScheme
  root.style.colorScheme = theme.colorScheme
  for (const [token, variable] of Object.entries(CSS_VARIABLES)) {
    root.style.setProperty(
      variable,
      theme.tokens[token as keyof ThemeTokens],
    )
  }
  root.style.setProperty(
    '--terminal-background',
    theme.terminal.background!,
  )
  root.style.setProperty('--terminal-text', theme.terminal.foreground!)
  root.style.setProperty('--terminal-text-muted', theme.tokens.textMuted)
  root.style.setProperty('--terminal-border', theme.tokens.textMuted)
  try {
    window.localStorage.setItem(THEME_STORAGE_KEY, theme.id)
  } catch {
    // Keep the selected theme live when browser storage is unavailable.
  }
  window.dispatchEvent(
    new CustomEvent<ThemeId>(THEME_CHANGE_EVENT, {
      detail: theme.id,
    }),
  )
}

export function terminalTheme(id: ThemeId): ITheme {
  return { ...themeDefinition(id).terminal }
}
