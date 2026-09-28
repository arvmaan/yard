// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { WorkerDispositionSheet } from './WorkerDispositionSheet'
import { activeAssignmentFixture } from './testSupport/assignmentFixture'

afterEach(cleanup)

describe('WorkerDispositionSheet', () => {
  it('asks how the work ended before ending the session', () => {
    const onConfirm = vi.fn(async () => undefined)
    const onClose = vi.fn()
    render(
      <WorkerDispositionSheet
        assignment={activeAssignmentFixture()}
        busy={null}
        error={null}
        mode="end"
        onClose={onClose}
        onConfirm={onConfirm}
      />,
    )
    const dialog = screen.getByRole('dialog', { name: 'End session' })
    expect(dialog.textContent).toContain('Ship the API.')
    expect(dialog.textContent).toContain(
      'The agent keeps running until its Herdr tab is closed.',
    )
    fireEvent.click(
      screen.getByRole('button', { name: 'Complete and end session' }),
    )
    expect(onConfirm).toHaveBeenLastCalledWith('complete')
    const cancel = screen.getByRole('button', { name: 'End without completion' })
    expect(
      document.getElementById(cancel.getAttribute('aria-describedby') ?? '')
        ?.textContent,
    ).toBe('Records the work as cancelled, not completed.')
    fireEvent.click(cancel)
    expect(onConfirm).toHaveBeenLastCalledWith('cancel')
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }))
    expect(onClose).toHaveBeenCalledTimes(1)
  })

  it('offers Complete and delete when deleting, and locks while busy', () => {
    const onConfirm = vi.fn(async () => undefined)
    render(
      <WorkerDispositionSheet
        assignment={activeAssignmentFixture()}
        busy="complete"
        error="Delete worker failed"
        mode="delete"
        onClose={vi.fn()}
        onConfirm={onConfirm}
      />,
    )
    expect(screen.getByRole('dialog', { name: 'Delete worker' })).toBeTruthy()
    const complete = screen.getByRole('button', { name: 'Complete and delete' })
    expect((complete as HTMLButtonElement).disabled).toBe(true)
    expect(
      (screen.getByRole('button', { name: 'End without completion' }) as HTMLButtonElement)
        .disabled,
    ).toBe(true)
    expect(screen.getByRole('alert').textContent).toContain('Delete worker failed')
    fireEvent.click(complete)
    expect(onConfirm).not.toHaveBeenCalled()
  })
})
