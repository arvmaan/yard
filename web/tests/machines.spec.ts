import { expect, test, type Locator, type Page } from '@playwright/test'

const a = '0123456789abcdef0123456789abcdef'
const b = 'abcdef0123456789abcdef0123456789'

const local = {
  endpoint: { kind: 'local' },
  label: 'Local',
  enabled: true,
  connection_state: 'reachable',
  capabilities: {
    inventory_read: true,
    mutations: true,
    terminal_streaming: true,
  },
  sessions: [{ name: 'alpha', is_default: true, observed_running: true }],
}

const machine = (
  id: string,
  label: string,
  state = 'reachable',
  enabled = true,
) => ({
  endpoint: { kind: 'machine', machine_id: id },
  label,
  enabled,
  connection_state: state,
  capabilities: {
    inventory_read: enabled && state === 'reachable',
    mutations: false,
    terminal_streaming: false,
  },
  sessions: [
    {
      name: 'default',
      is_default: true,
      observed_running: state === 'reachable' ? true : null,
    },
  ],
})

const inventory = (id: string) => ({
  endpoint: { kind: 'machine', machine_id: id },
  inventory: {
    adapter: 'herdr',
    session: 'default',
    runtime_version: '0.9.3',
    protocol: 19,
    observed_at_unix_ms: 1,
    focus: {
      workspace_id: 'w1',
      tab_id: 'w1:t1',
      pane_id: 'w1:p1',
    },
    workspaces: [
      {
        runtime_id: 'w1',
        order: 1,
        label: 'Remote workspace',
        focused: true,
        active_tab_id: 'w1:t1',
        pane_count: 1,
        tab_count: 1,
        status: 'idle',
        tokens: {},
        worktree: null,
      },
    ],
    tabs: [
      {
        runtime_id: 'w1:t1',
        workspace_id: 'w1',
        order: 1,
        label: 'Remote tab',
        focused: true,
        pane_count: 1,
        status: 'idle',
      },
    ],
    panes: [
      {
        runtime_id: 'w1:p1',
        terminal_id: 'remote-terminal',
        workspace_id: 'w1',
        tab_id: 'w1:t1',
        focused: true,
        cwd: '/remote',
        foreground_cwd: '/remote',
        label: 'Remote pane',
        provider: 'codex',
        display_provider: 'Codex',
        status: 'idle',
        tokens: {},
        provider_session: null,
        revision: '1',
      },
    ],
    workers: [
      {
        runtime_id: 'remote-agent',
        terminal_id: 'remote-terminal',
        workspace_id: 'w1',
        tab_id: 'w1:t1',
        pane_id: 'w1:p1',
        name: 'remote-agent',
        provider: 'codex',
        display_provider: 'Codex',
        status: 'idle',
        focused: true,
        launch_pending: false,
        interactive_ready: true,
        state_change_sequence: '1',
        cwd: '/remote',
        foreground_cwd: '/remote',
        tokens: {},
        provider_session: null,
        revision: '1',
      },
    ],
    child_agents: [],
  },
})

async function setup(page: Page, initialEndpoints?: unknown[]) {
  let endpoints =
    initialEndpoints ?? [
      local,
      machine(a, 'Build box'),
      machine(b, 'Offline box', 'authentication_required'),
    ]
  let fail = false

  await page.route('**/api/v1/runtimes/herdr/endpoints', (route) =>
    route.fulfill({ json: { adapter: 'herdr', endpoints } }),
  )
  await page.route(
    '**/api/v1/runtimes/herdr/machines/*/inventory',
    (route) =>
      fail
        ? route.fulfill({
            status: 502,
            json: {
              error: {
                code: 'machine_inventory_unavailable',
                message: 'private SSH detail',
              },
            },
          })
        : route.fulfill({
            json: inventory(
              decodeURIComponent(
                new URL(route.request().url()).pathname.split('/').at(-2) ??
                  '',
              ),
            ),
          }),
  )

  return {
    rename() {
      endpoints = [local, machine(a, 'Renamed box')]
    },
    remove() {
      endpoints = [local, machine(b, 'Offline box', 'incompatible')]
    },
    fail() {
      fail = true
    },
  }
}

async function waitForLocalSession(page: Page) {
  await expect(
    page.getByRole('button', { name: /^Runtime health:/ }),
  ).toHaveAccessibleName('Runtime health: alpha, Herdr observed')
}

async function openMachines(page: Page) {
  await page.getByRole('button', { name: /^Runtime health:/ }).click()
  const health = page.getByRole('dialog', { name: 'Runtime health' })
  await health.getByRole('button', { name: 'Machines' }).click()
  await expect(health).toHaveCount(0)

  const dialog = page.getByRole('dialog', { name: 'Machines' })
  await expect(dialog).toHaveAttribute('id', 'machines-dialog')
  return dialog
}

async function expectNoRemoteControls(details: Locator) {
  await expect(
    details.getByRole('button', {
      name: /terminal|manage|allocate|assign|launch|start|stop|restart|delete|remove|cleanup|kill|focus/i,
    }),
  ).toHaveCount(0)
  await expect(details.locator('a, input, select, textarea, form')).toHaveCount(
    0,
  )
  await expect(details.getByRole('button')).toHaveCount(1)
  await expect(
    details.getByRole('button', { name: 'Refresh inventory' }),
  ).toBeVisible()
}

test('remote inventory is observed topology without mutation controls', async ({
  page,
}) => {
  await setup(page)
  await page.goto('/')
  await waitForLocalSession(page)

  const dialog = await openMachines(page)
  await expect(dialog.getByRole('button', { name: /Local/ })).toBeVisible()
  await dialog.getByRole('button', { name: /Build box/ }).click()

  const details = dialog.locator('.machines-dialog__details')
  await expect(details.getByText('Observed / read-only.')).toBeVisible()
  await expect(details.getByText('Remote workspace')).toBeVisible()
  await expect(details.getByText(/Observed agent: remote-agent/)).toBeVisible()
  await expectNoRemoteControls(details)
})

test('refresh preserves stable identity and never falls back to Local', async ({
  page,
}) => {
  const controls = await setup(page)
  await page.goto('/')
  await waitForLocalSession(page)

  const dialog = await openMachines(page)
  await expect(dialog.getByRole('button', { name: /Local/ })).toBeVisible()
  await dialog.getByRole('button', { name: /Build box/ }).click()
  await expect(dialog.getByText('Remote workspace')).toBeVisible()

  controls.rename()
  await dialog.getByRole('button', { name: 'Refresh saved machines' }).click()
  await expect(
    dialog.getByRole('button', { name: /Renamed box/ }),
  ).toHaveAttribute('aria-pressed', 'true')

  controls.fail()
  await dialog.getByRole('button', { name: 'Refresh inventory' }).click()
  await expect(dialog.getByRole('alert')).toContainText(
    'did not fall back to Local',
  )

  controls.remove()
  await dialog.getByRole('button', { name: 'Refresh saved machines' }).click()
  await expect(dialog.getByRole('button', { name: /Local/ })).toHaveAttribute(
    'aria-pressed',
    'true',
  )
  await dialog.getByRole('button', { name: /Offline box/ }).click()
  await expect(dialog.getByText(`herdr machine reconnect '${b}'`)).toBeVisible()

  await page.keyboard.press('Escape')
  const healthTrigger = page.getByRole('button', {
    name: /^Runtime health:/,
  })
  await expect(healthTrigger).toBeFocused()
  await expect(healthTrigger).toHaveAccessibleName(
    'Runtime health: alpha, Herdr observed',
  )
})

test('shows an explicit empty state when Herdr returns no endpoints', async ({
  page,
}) => {
  await setup(page, [])
  await page.goto('/')
  await waitForLocalSession(page)

  const dialog = await openMachines(page)
  await expect(
    dialog.getByText('No saved endpoints were returned by Herdr.'),
  ).toBeVisible()
  await expect(dialog.getByText('Select a machine.')).toBeVisible()
})

test('add handoff traps focus, resets target, escapes arguments, and fits 390px', async ({
  page,
}) => {
  await setup(page)
  await page.emulateMedia({ reducedMotion: 'reduce' })
  await page.setViewportSize({ width: 390, height: 844 })
  await page.goto('/')
  await waitForLocalSession(page)

  const dialog = await openMachines(page)
  await expect(dialog.getByRole('button', { name: /Local/ })).toBeVisible()
  expect(
    await dialog.evaluate((element) => element.scrollWidth - element.clientWidth),
  ).toBeLessThanOrEqual(0)
  await expect(dialog).toHaveCSS('animation-name', 'none')
  await expect(dialog).toHaveCSS('transition-duration', '0s')

  await dialog.getByRole('button', { name: 'Add machine' }).click()
  const add = page.getByRole('dialog', { name: 'Add saved machine' })
  const target = add.getByLabel('SSH target')
  await expect(target).toBeFocused()
  await expect(add).toHaveCSS('animation-name', 'none')
  await expect(add).toHaveCSS('transition-duration', '0s')
  await target.fill('-host name')
  await add.getByLabel('Label (optional)').fill("Builder's box")
  await add.getByLabel('Remote session (optional)').fill('release one')
  await add.getByRole('button', { name: 'Prepare command' }).click()
  await expect(
    add.getByText(
      `herdr machine add --label='Builder'"'"'s box' --remote-session='release one' -- '-host name'`,
    ),
  ).toBeVisible()
  await expect(add.getByText(/copying does not execute/i)).toBeVisible()
  expect(
    await add.evaluate((element) => element.scrollWidth - element.clientWidth),
  ).toBeLessThanOrEqual(0)

  await page.keyboard.press('Escape')
  await expect(dialog.getByRole('button', { name: 'Add machine' })).toBeFocused()
  await dialog.getByRole('button', { name: 'Add machine' }).click()
  await expect(
    page.getByRole('dialog', { name: 'Add saved machine' }).getByLabel('SSH target'),
  ).toHaveValue('')
})
