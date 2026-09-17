import { describe, expect, it } from 'vitest'
import {
  readTheme,
  THEME_OPTIONS,
  THEME_REGISTRY,
  themeDefinition,
  terminalTheme,
} from './theme'

function luminance(color: string) {
  const channels = color
    .slice(1)
    .match(/.{2}/g)!
    .map((channel) => Number.parseInt(channel, 16) / 255)
    .map((channel) =>
      channel <= 0.04045
        ? channel / 12.92
        : ((channel + 0.055) / 1.055) ** 2.4,
    )
  return 0.2126 * channels[0] + 0.7152 * channels[1] + 0.0722 * channels[2]
}

function contrast(first: string, second: string) {
  const firstLuminance = luminance(first)
  const secondLuminance = luminance(second)
  return (
    (Math.max(firstLuminance, secondLuminance) + 0.05) /
    (Math.min(firstLuminance, secondLuminance) + 0.05)
  )
}

function environment(value: string | null, prefersDark: boolean) {
  return {
    localStorage: { getItem: () => value },
    matchMedia: () => ({ matches: prefersDark }),
  }
}

describe('theme registry', () => {
  it('resolves every built-in option through stable unique IDs', () => {
    expect(new Set(THEME_OPTIONS.map(({ id }) => id)).size).toBe(
      THEME_OPTIONS.length,
    )
    for (const option of THEME_OPTIONS) {
      const theme = THEME_REGISTRY.get(option.id)
      expect(theme?.label).toBe(option.label)
      expect(theme?.terminal.background).toBeTruthy()
      expect(theme?.tokens.surfaceCanvas).toBeTruthy()
    }
  })

  it('selects stored IDs and falls back explicitly', () => {
    expect(readTheme(environment('nord', false))).toBe('nord')
    expect(readTheme(environment('missing-theme', true))).toBe('dark')
    expect(readTheme(environment(null, false))).toBe('light')
    expect(themeDefinition('missing-theme').id).toBe('light')
  })

  it('guards throwing browser storage and preference access', () => {
    expect(
      readTheme({
        get localStorage(): Pick<Storage, 'getItem'> {
          throw new Error('storage unavailable')
        },
        matchMedia: () => ({ matches: true }),
      }),
    ).toBe('dark')
    expect(
      readTheme({
        localStorage: { getItem: () => null },
        get matchMedia(): (
          query: string,
        ) => Pick<MediaQueryList, 'matches'> {
          throw new Error('preference unavailable')
        },
      }),
    ).toBe('light')
  })

  it('returns a fresh xterm theme for the selected app theme', () => {
    const first = terminalTheme('solarized-light')
    const second = terminalTheme('solarized-light')
    expect(first).not.toBe(second)
    expect(first.background).toBe('#fdf6e3')
    expect(first.foreground).toBe('#586e75')
  })

  it('keeps app and terminal text readable for every built-in theme', () => {
    for (const theme of THEME_REGISTRY.values()) {
      expect(
        contrast(theme.tokens.textPrimary, theme.tokens.surfacePanel),
        theme.id,
      ).toBeGreaterThanOrEqual(4.5)
      expect(
        contrast(
          theme.tokens.statusWarningForeground,
          theme.tokens.statusWarningSoft,
        ),
        `${theme.id} warning`,
      ).toBeGreaterThanOrEqual(4.5)
      expect(
        contrast(theme.tokens.textMuted, theme.tokens.surfaceMuted),
        `${theme.id} neutral status`,
      ).toBeGreaterThanOrEqual(4.5)
      expect(
        contrast(theme.tokens.textMuted, theme.terminal.background!),
        `${theme.id} terminal chrome`,
      ).toBeGreaterThanOrEqual(4.5)
      expect(
        contrast(theme.terminal.foreground!, theme.terminal.background!),
        `${theme.id} terminal text`,
      ).toBeGreaterThanOrEqual(4.5)
      expect(
        contrast(
          theme.terminal.selectionForeground ?? theme.terminal.foreground!,
          theme.terminal.selectionBackground!,
        ),
        `${theme.id} terminal selection`,
      ).toBeGreaterThanOrEqual(4.5)
    }
  })

  it('separates Solarized panels and strong borders without changing layout', () => {
    for (const id of ['solarized-dark', 'solarized-light']) {
      const theme = themeDefinition(id)
      expect(theme.tokens.surfacePanel).not.toBe(theme.tokens.surfaceCanvas)
      expect(
        contrast(theme.tokens.borderStrong, theme.tokens.surfacePanel),
        id + ' border',
      ).toBeGreaterThanOrEqual(3)
    }
  })
})
