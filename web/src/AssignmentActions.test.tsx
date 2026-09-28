// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { AssignmentActions } from './AssignmentActions'
import { activeAssignmentFixture } from './testSupport/assignmentFixture'

afterEach(cleanup)

function renderActions(overrides: Partial<Parameters<typeof AssignmentActions>[0]> = {}) {
  const props = {
    assignment: activeAssignmentFixture(),
    busy: false,
    completeAndEndSession: true,
    error: null,
    onComplete: vi.fn(),
    onCompleteAndEndSessionChange: vi.fn(),
    onCompleteWithDetails: vi.fn(),
    onDelete: vi.fn(),
    onEndSession: vi.fn(),
    onUndo: vi.fn(),
    pending: false,
    ...overrides,
  }
  render(<AssignmentActions {...props} />)
  return props
}

describe('AssignmentActions', () => {
  it('offers one primary Complete whose description names the objective', () => {
    const props = renderActions()
    const complete = screen.getByRole('button', { name: 'Complete' })
    expect(complete.getAttribute('aria-describedby')).toBeTruthy()
    const description = document.getElementById(
      complete.getAttribute('aria-describedby') ?? '',
    )
    expect(description?.textContent).toContain('Ship the API.')
    expect(description?.textContent).toContain('ends the session')
    expect(screen.queryByText('Record completion')).toBeNull()

    fireEvent.click(complete)
    expect(props.onComplete).toHaveBeenCalledTimes(1)
  })

  it('shows Completing… with Undo during the pre-commit window', () => {
    const props = renderActions({ pending: true })
    expect(screen.queryByRole('button', { name: 'Complete' })).toBeNull()
    expect(screen.getByRole('status').textContent).toContain('Completing…')
    fireEvent.click(screen.getByRole('button', { name: 'Undo' }))
    expect(props.onUndo).toHaveBeenCalledTimes(1)
  })

  it('keeps detailed completion, End session, and Delete worker in an inline overflow', () => {
    const props = renderActions({ completeAndEndSession: false })
    const toggle = screen.getByRole('button', { name: 'More worker actions' })
    expect(toggle.getAttribute('aria-expanded')).toBe('false')
    expect(screen.queryByRole('group', { name: 'More worker actions' })).toBeNull()

    fireEvent.click(toggle)
    expect(toggle.getAttribute('aria-expanded')).toBe('true')
    const overflow = screen.getByRole('group', { name: 'More worker actions' })
    expect(overflow.id).toBe(toggle.getAttribute('aria-controls'))
    const preference = screen.getByRole('checkbox', {
      name: /Complete and end session/,
    }) as HTMLInputElement
    expect(preference.checked).toBe(false)
    fireEvent.click(preference)
    expect(props.onCompleteAndEndSessionChange).toHaveBeenCalledWith(true)

    fireEvent.click(screen.getByRole('button', { name: 'End session' }))
    expect(props.onEndSession).toHaveBeenCalledTimes(1)
    expect(screen.queryByRole('group', { name: 'More worker actions' })).toBeNull()

    fireEvent.click(toggle)
    fireEvent.click(screen.getByRole('button', { name: 'Complete with details…' }))
    expect(props.onCompleteWithDetails).toHaveBeenCalledTimes(1)

    fireEvent.click(toggle)
    fireEvent.click(screen.getByRole('button', { name: 'Delete worker' }))
    expect(props.onDelete).toHaveBeenCalledTimes(1)

    fireEvent.click(toggle)
    fireEvent.keyDown(screen.getByRole('group', { name: 'More worker actions' }), {
      key: 'Escape',
    })
    expect(screen.queryByRole('group', { name: 'More worker actions' })).toBeNull()
  })

  it('shows a failed completion with Retry', () => {
    const onRetry = vi.fn()
    renderActions({ error: 'Yard is unreachable', onRetry })
    expect(screen.getByRole('alert').textContent).toContain('Yard is unreachable')
    fireEvent.click(screen.getByRole('button', { name: 'Retry' }))
    expect(onRetry).toHaveBeenCalledTimes(1)
  })

  it('moves focus to Undo when the window starts and back to Complete after Undo', () => {
    const props = {
      assignment: activeAssignmentFixture(),
      busy: false,
      completeAndEndSession: true,
      error: null,
      onComplete: vi.fn(),
      onCompleteAndEndSessionChange: vi.fn(),
      onCompleteWithDetails: vi.fn(),
      onUndo: vi.fn(),
      pending: false,
    }
    const view = render(<AssignmentActions {...props} />)
    const complete = screen.getByRole('button', { name: 'Complete' })
    complete.focus()
    fireEvent.click(complete)
    view.rerender(<AssignmentActions {...props} pending />)
    const undo = screen.getByRole('button', { name: 'Undo' })
    expect(document.activeElement).toBe(undo)

    fireEvent.click(undo)
    view.rerender(<AssignmentActions {...props} pending={false} />)
    expect(document.activeElement).toBe(
      screen.getByRole('button', { name: 'Complete' }),
    )
  })
})
