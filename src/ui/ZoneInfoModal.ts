/**
 * Zone Info Modal - Displays detailed information about a session/zone
 *
 * Shows session stats, git status, token usage, files touched, targets, etc.
 */

import type { ManagedSession, GitStatus } from '../../shared/types'
import type { SessionAPI, TentaclesTarget } from '../api/SessionAPI'
import { soundManager } from '../audio'
import { formatTimeAgo } from './FeedManager'

// ============================================================================
// Types
// ============================================================================

export interface ZoneInfoData {
  /** The managed session data */
  managedSession: ManagedSession
  /** Session-specific stats from main.ts state */
  stats?: {
    toolsUsed: number
    filesTouched: Set<string>
    activeSubagents: number
  }
}

// ============================================================================
// State
// ============================================================================

let modal: HTMLElement | null = null
let soundEnabled = true
let sessionAPI: SessionAPI | null = null
let currentSessionId: string | null = null

// ============================================================================
// Public API
// ============================================================================

/**
 * Initialize the zone info modal
 */
export function setupZoneInfoModal(options: { soundEnabled: boolean; sessionAPI?: SessionAPI }): void {
  soundEnabled = options.soundEnabled
  if (options.sessionAPI) sessionAPI = options.sessionAPI
  modal = document.getElementById('zone-info-modal')

  const closeBtn = document.getElementById('zone-info-close')
  closeBtn?.addEventListener('click', hideZoneInfoModal)

  // Close on backdrop click
  modal?.addEventListener('click', (e) => {
    if (e.target === modal) {
      hideZoneInfoModal()
    }
  })

  // Close on Escape
  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape' && modal?.classList.contains('visible')) {
      hideZoneInfoModal()
    }
  })
}

/**
 * Show the zone info modal with session data
 */
export function showZoneInfoModal(data: ZoneInfoData): void {
  if (!modal) return

  if (soundEnabled) {
    soundManager.play('modal_open')
  }

  currentSessionId = data.managedSession.id
  renderContent(data)
  modal.classList.add('visible')

  // Fetch targets asynchronously
  fetchAndRenderTargets(data.managedSession.id)
}

/**
 * Hide the zone info modal
 */
export function hideZoneInfoModal(): void {
  if (!modal) return

  if (soundEnabled) {
    soundManager.play('modal_cancel')
  }

  modal.classList.remove('visible')
}

/**
 * Update sound enabled state
 */
export function setZoneInfoSoundEnabled(enabled: boolean): void {
  soundEnabled = enabled
}

// ============================================================================
// Rendering
// ============================================================================

function renderContent(data: ZoneInfoData): void {
  const content = document.getElementById('zone-info-content')
  if (!content) return

  const { managedSession: s, stats } = data
  const filesTouched = stats?.filesTouched ? Array.from(stats.filesTouched) : []

  content.innerHTML = `
    <!-- Header -->
    <div class="zone-info-header">
      <div class="zone-info-name">${escapeHtml(s.name)}</div>
      <div class="zone-info-status zone-info-status--${s.status}">${s.status}</div>
    </div>

    <!-- Basic Info -->
    <div class="zone-info-section">
      <div class="zone-info-row">
        <span class="zone-info-label">Directory</span>
        <span class="zone-info-value zone-info-mono">${escapeHtml(s.cwd || '~')}</span>
      </div>
      <div class="zone-info-row">
        <span class="zone-info-label">tmux Session</span>
        <span class="zone-info-value zone-info-mono">${escapeHtml(s.tmuxSession)}</span>
      </div>
      <div class="zone-info-row">
        <span class="zone-info-label">Created</span>
        <span class="zone-info-value">${formatTimeAgo(s.createdAt)}</span>
      </div>
      <div class="zone-info-row">
        <span class="zone-info-label">Last Activity</span>
        <span class="zone-info-value">${formatTimeAgo(s.lastActivity)}</span>
      </div>
      ${s.currentTool ? `
      <div class="zone-info-row">
        <span class="zone-info-label">Current Tool</span>
        <span class="zone-info-value zone-info-highlight">${escapeHtml(s.currentTool)}</span>
      </div>
      ` : ''}
    </div>

    <!-- Stats -->
    <div class="zone-info-section">
      <div class="zone-info-section-title">Statistics</div>
      <div class="zone-info-stats-grid">
        <div class="zone-info-stat">
          <div class="zone-info-stat-value">${stats?.toolsUsed ?? 0}</div>
          <div class="zone-info-stat-label">Tools Used</div>
        </div>
        <div class="zone-info-stat">
          <div class="zone-info-stat-value">${filesTouched.length}</div>
          <div class="zone-info-stat-label">Files Touched</div>
        </div>
        <div class="zone-info-stat">
          <div class="zone-info-stat-value">${stats?.activeSubagents ?? 0}</div>
          <div class="zone-info-stat-label">Subagents</div>
        </div>
      </div>
    </div>

    <!-- Tokens -->
    ${s.tokens ? `
    <div class="zone-info-section">
      <div class="zone-info-section-title">Token Usage</div>
      <div class="zone-info-tokens">
        <div class="zone-info-token-row">
          <span>Current Conversation</span>
          <span class="zone-info-token-value">${formatNumber(s.tokens.current)}</span>
        </div>
        <div class="zone-info-token-row">
          <span>Cumulative (Session)</span>
          <span class="zone-info-token-value">${formatNumber(s.tokens.cumulative)}</span>
        </div>
      </div>
    </div>
    ` : ''}

    <!-- Git Status -->
    ${s.gitStatus?.isRepo ? renderGitStatus(s.gitStatus) : `
    <div class="zone-info-section">
      <div class="zone-info-section-title">Git Status</div>
      <div class="zone-info-muted">Not a git repository</div>
    </div>
    `}

    <!-- Files Touched -->
    ${filesTouched.length > 0 ? `
    <div class="zone-info-section">
      <div class="zone-info-section-title">Files Touched (${filesTouched.length})</div>
      <div class="zone-info-files">
        ${filesTouched.slice(0, 10).map(f => `
          <div class="zone-info-file">${escapeHtml(shortenPath(f))}</div>
        `).join('')}
        ${filesTouched.length > 10 ? `
          <div class="zone-info-file zone-info-muted">... and ${filesTouched.length - 10} more</div>
        ` : ''}
      </div>
    </div>
    ` : ''}

    <!-- Tentacles Targets (populated async) -->
    <div class="zone-info-section zone-info-targets-section" id="zone-info-targets" style="display: none;">
      <div class="zone-info-section-title">Tentacles Targets</div>
      <div id="zone-info-targets-list"></div>
      <div class="zone-info-targets-actions" id="zone-info-targets-actions"></div>
    </div>

    <!-- IDs (for debugging) -->
    <div class="zone-info-section zone-info-ids">
      <div class="zone-info-section-title">Identifiers</div>
      <div class="zone-info-row">
        <span class="zone-info-label">Managed ID</span>
        <span class="zone-info-value zone-info-mono zone-info-small">${s.id}</span>
      </div>
      ${s.claudeSessionId ? `
      <div class="zone-info-row">
        <span class="zone-info-label">Claude Session</span>
        <span class="zone-info-value zone-info-mono zone-info-small">${s.claudeSessionId}</span>
      </div>
      ` : ''}
    </div>
  `
}

function renderGitStatus(git: GitStatus): string {
  const stagedTotal = git.staged.added + git.staged.modified + git.staged.deleted
  const unstagedTotal = git.unstaged.added + git.unstaged.modified + git.unstaged.deleted
  const isDirty = stagedTotal > 0 || unstagedTotal > 0 || git.untracked > 0

  return `
    <div class="zone-info-section">
      <div class="zone-info-section-title">Git Status</div>

      <!-- Branch -->
      <div class="zone-info-git-branch">
        <span class="zone-info-branch-icon">⎇</span>
        <span class="zone-info-branch-name">${escapeHtml(git.branch)}</span>
        ${git.ahead > 0 ? `<span class="zone-info-branch-ahead">↑${git.ahead}</span>` : ''}
        ${git.behind > 0 ? `<span class="zone-info-branch-behind">↓${git.behind}</span>` : ''}
        ${isDirty ? `<span class="zone-info-branch-dirty">●</span>` : `<span class="zone-info-branch-clean">✓</span>`}
      </div>

      <!-- Changes -->
      ${stagedTotal > 0 ? `
      <div class="zone-info-git-changes">
        <span class="zone-info-changes-label">Staged</span>
        <span class="zone-info-changes-detail">
          ${git.staged.added > 0 ? `<span class="zone-info-added">+${git.staged.added}</span>` : ''}
          ${git.staged.modified > 0 ? `<span class="zone-info-modified">~${git.staged.modified}</span>` : ''}
          ${git.staged.deleted > 0 ? `<span class="zone-info-deleted">-${git.staged.deleted}</span>` : ''}
        </span>
      </div>
      ` : ''}

      ${unstagedTotal > 0 ? `
      <div class="zone-info-git-changes">
        <span class="zone-info-changes-label">Unstaged</span>
        <span class="zone-info-changes-detail">
          ${git.unstaged.added > 0 ? `<span class="zone-info-added">+${git.unstaged.added}</span>` : ''}
          ${git.unstaged.modified > 0 ? `<span class="zone-info-modified">~${git.unstaged.modified}</span>` : ''}
          ${git.unstaged.deleted > 0 ? `<span class="zone-info-deleted">-${git.unstaged.deleted}</span>` : ''}
        </span>
      </div>
      ` : ''}

      ${git.untracked > 0 ? `
      <div class="zone-info-git-changes">
        <span class="zone-info-changes-label">Untracked</span>
        <span class="zone-info-changes-detail zone-info-muted">${git.untracked} files</span>
      </div>
      ` : ''}

      ${!isDirty ? `
      <div class="zone-info-git-clean">Working tree clean</div>
      ` : ''}

      <!-- Lines changed -->
      ${(git.linesAdded > 0 || git.linesRemoved > 0) ? `
      <div class="zone-info-git-lines">
        ${git.linesAdded > 0 ? `<span class="zone-info-added">+${git.linesAdded}</span>` : ''}
        ${git.linesRemoved > 0 ? `<span class="zone-info-deleted">-${git.linesRemoved}</span>` : ''}
        <span class="zone-info-muted">lines</span>
      </div>
      ` : ''}

      <!-- Last commit -->
      ${git.lastCommitMessage ? `
      <div class="zone-info-git-commit">
        <span class="zone-info-commit-msg">${escapeHtml(git.lastCommitMessage)}</span>
        ${git.lastCommitTime ? `
        <span class="zone-info-commit-time">${formatTimeAgo(git.lastCommitTime * 1000)}</span>
        ` : ''}
      </div>
      ` : ''}
    </div>
  `
}

// ============================================================================
// Targets Management
// ============================================================================

async function fetchAndRenderTargets(sessionId: string): Promise<void> {
  if (!sessionAPI) return

  const section = document.getElementById('zone-info-targets')
  const list = document.getElementById('zone-info-targets-list')
  const actions = document.getElementById('zone-info-targets-actions')
  if (!section || !list || !actions) return

  const result = await sessionAPI.listTargets(sessionId)
  if (!result.ok || !result.targets) {
    // Tentacles not enabled for this session — hide section
    section.style.display = 'none'
    return
  }

  section.style.display = ''
  renderTargetsList(list, result.targets, sessionId)
  renderTargetsActions(actions, sessionId)
}

function renderTargetsList(container: HTMLElement, targets: TentaclesTarget[], sessionId: string): void {
  if (targets.length === 0) {
    container.innerHTML = '<div class="zone-info-muted">No targets configured</div>'
    return
  }

  container.innerHTML = targets.map(t => `
    <div class="zone-info-target">
      <div class="zone-info-target-header">
        <span class="zone-info-target-name">${escapeHtml(t.name)}</span>
        <span class="zone-info-target-type zone-info-target-type--${t.targetType}">${escapeHtml(t.targetType)}</span>
        <button class="zone-info-target-remove" data-target-name="${escapeHtml(t.name)}" title="Remove target">&times;</button>
      </div>
      ${renderTargetParams(t)}
    </div>
  `).join('')

  // Wire up remove buttons
  container.querySelectorAll('.zone-info-target-remove').forEach(btn => {
    btn.addEventListener('click', async () => {
      const name = (btn as HTMLElement).dataset.targetName
      if (!name || !sessionAPI) return
      const result = await sessionAPI.removeTarget(sessionId, name)
      if (result.ok) {
        fetchAndRenderTargets(sessionId)
      }
    })
  })
}

function renderTargetParams(target: TentaclesTarget): string {
  if (!target.params || Object.keys(target.params).length === 0) return ''

  const params = target.params
  const rows: string[] = []

  if (params.container) rows.push(`<span class="zone-info-label">Container</span><span class="zone-info-value zone-info-mono">${escapeHtml(String(params.container))}</span>`)
  if (params.image) rows.push(`<span class="zone-info-label">Image</span><span class="zone-info-value zone-info-mono">${escapeHtml(String(params.image))}</span>`)
  if (params.dockerfile) rows.push(`<span class="zone-info-label">Dockerfile</span><span class="zone-info-value zone-info-mono">${escapeHtml(String(params.dockerfile))}</span>`)
  if (params.host) rows.push(`<span class="zone-info-label">Host</span><span class="zone-info-value zone-info-mono">${escapeHtml(String(params.host))}</span>`)
  if (params.mode) rows.push(`<span class="zone-info-label">Mode</span><span class="zone-info-value">${escapeHtml(String(params.mode))}</span>`)

  if (params.editable) rows.push(`<span class="zone-info-label">Editable</span><span class="zone-info-value">Yes — agent can modify Dockerfile</span>`)

  if (Array.isArray(params.volumes) && params.volumes.length > 0) {
    rows.push(`<span class="zone-info-label">Volumes</span><span class="zone-info-value zone-info-mono">${(params.volumes as string[]).map(v => escapeHtml(v)).join('<br>')}</span>`)
  }

  if (rows.length === 0) return ''
  return `<div class="zone-info-target-params">${rows.map(r => `<div class="zone-info-row">${r}</div>`).join('')}</div>`
}

function renderTargetsActions(container: HTMLElement, sessionId: string): void {
  container.innerHTML = `
    <div class="zone-info-targets-buttons">
      <button class="zone-info-target-add-btn" id="zone-info-add-host">+ Host</button>
      <button class="zone-info-target-add-btn" id="zone-info-add-container">+ Container</button>
    </div>
    <div id="zone-info-add-form" style="display: none;"></div>
  `

  document.getElementById('zone-info-add-host')?.addEventListener('click', async () => {
    if (!sessionAPI) return
    const name = prompt('Host target name:')
    if (!name) return
    const result = await sessionAPI.addTarget(sessionId, { name, targetType: 'host' })
    if (result.ok) {
      fetchAndRenderTargets(sessionId)
    }
  })

  document.getElementById('zone-info-add-container')?.addEventListener('click', () => {
    showAddContainerForm(sessionId)
  })
}

function showAddContainerForm(sessionId: string): void {
  const form = document.getElementById('zone-info-add-form')
  if (!form) return

  form.style.display = ''
  form.innerHTML = `
    <div class="zone-info-add-container-form">
      <div class="zone-info-row">
        <label class="zone-info-label" for="zi-target-name">Name</label>
        <input id="zi-target-name" class="zone-info-input" placeholder="my-container" />
      </div>
      <div class="zone-info-add-mode">
        <label class="tentacles-radio"><input type="radio" name="zi-container-mode" value="container" checked /> Existing</label>
        <label class="tentacles-radio"><input type="radio" name="zi-container-mode" value="image" /> From Image</label>
        <label class="tentacles-radio"><input type="radio" name="zi-container-mode" value="dockerfile" /> From Dockerfile</label>
      </div>
      <div id="zi-mode-fields">
        <div class="zone-info-row">
          <label class="zone-info-label" for="zi-container-name">Container</label>
          <input id="zi-container-name" class="zone-info-input" placeholder="container name" />
        </div>
      </div>
      <div id="zi-volumes" style="display: none;">
        <div class="zone-info-label" style="margin-bottom: 4px;">Volumes</div>
        <div id="zi-volumes-list"></div>
        <button class="zone-info-target-add-btn zone-info-target-add-btn--small" id="zi-add-volume">+ Volume</button>
      </div>
      <div class="zone-info-targets-buttons" style="margin-top: 8px;">
        <button class="zone-info-target-add-btn zone-info-target-add-btn--primary" id="zi-submit-target">Add</button>
        <button class="zone-info-target-add-btn" id="zi-cancel-target">Cancel</button>
      </div>
    </div>
  `

  const modeRadios = form.querySelectorAll('input[name="zi-container-mode"]')
  const modeFields = document.getElementById('zi-mode-fields')!
  const volumesSection = document.getElementById('zi-volumes')!

  function updateModeFields(): void {
    const mode = (form!.querySelector('input[name="zi-container-mode"]:checked') as HTMLInputElement)?.value
    if (mode === 'container') {
      modeFields.innerHTML = `
        <div class="zone-info-row">
          <label class="zone-info-label" for="zi-container-name">Container</label>
          <input id="zi-container-name" class="zone-info-input" placeholder="container name" />
        </div>`
      volumesSection.style.display = 'none'
    } else if (mode === 'image') {
      modeFields.innerHTML = `
        <div class="zone-info-row">
          <label class="zone-info-label" for="zi-image-name">Image</label>
          <input id="zi-image-name" class="zone-info-input" placeholder="ubuntu:24.04" />
        </div>`
      volumesSection.style.display = ''
    } else if (mode === 'dockerfile') {
      modeFields.innerHTML = `
        <div class="zone-info-row">
          <label class="zone-info-label" for="zi-dockerfile-path">Dockerfile</label>
          <input id="zi-dockerfile-path" class="zone-info-input" placeholder="/path/to/Dockerfile" />
        </div>
        <div class="zone-info-row">
          <label class="tentacles-radio"><input type="checkbox" id="zi-editable-checkbox" /> Allow agent to modify Dockerfile</label>
        </div>`
      volumesSection.style.display = ''
    }
  }

  modeRadios.forEach(r => r.addEventListener('change', updateModeFields))

  document.getElementById('zi-add-volume')?.addEventListener('click', () => {
    const list = document.getElementById('zi-volumes-list')!
    const row = document.createElement('div')
    row.className = 'tentacles-volume-row'
    row.innerHTML = `
      <input class="zi-vol-host" placeholder="/host/path" />
      <span class="tentacles-vol-arrow">:</span>
      <input class="zi-vol-container" placeholder="/container/path" />
      <button class="zone-info-target-remove" title="Remove">&times;</button>
    `
    row.querySelector('.zone-info-target-remove')?.addEventListener('click', () => row.remove())
    list.appendChild(row)
  })

  document.getElementById('zi-cancel-target')?.addEventListener('click', () => {
    form.style.display = 'none'
    form.innerHTML = ''
  })

  document.getElementById('zi-submit-target')?.addEventListener('click', async () => {
    if (!sessionAPI) return
    const name = (document.getElementById('zi-target-name') as HTMLInputElement)?.value.trim()
    if (!name) return

    const mode = (form.querySelector('input[name="zi-container-mode"]:checked') as HTMLInputElement)?.value
    const params: Record<string, unknown> = { mode }

    if (mode === 'container') {
      params.container = (document.getElementById('zi-container-name') as HTMLInputElement)?.value.trim()
      if (!params.container) return
    } else if (mode === 'image') {
      params.image = (document.getElementById('zi-image-name') as HTMLInputElement)?.value.trim()
      if (!params.image) return
      params.volumes = collectVolumes()
    } else if (mode === 'dockerfile') {
      params.dockerfile = (document.getElementById('zi-dockerfile-path') as HTMLInputElement)?.value.trim()
      if (!params.dockerfile) return
      params.volumes = collectVolumes()
      const editable = (document.getElementById('zi-editable-checkbox') as HTMLInputElement)?.checked
      if (editable) params.editable = true
    }

    const result = await sessionAPI.addTarget(sessionId, { name, targetType: 'container', params })
    if (result.ok) {
      form.style.display = 'none'
      form.innerHTML = ''
      fetchAndRenderTargets(sessionId)
    }
  })
}

function collectVolumes(): string[] {
  const volumes: string[] = []
  document.querySelectorAll('#zi-volumes-list .tentacles-volume-row').forEach(row => {
    const host = (row.querySelector('.zi-vol-host') as HTMLInputElement)?.value.trim()
    const container = (row.querySelector('.zi-vol-container') as HTMLInputElement)?.value.trim()
    if (host && container) volumes.push(`${host}:${container}`)
  })
  return volumes
}

// ============================================================================
// Utilities
// ============================================================================

function escapeHtml(text: string): string {
  const div = document.createElement('div')
  div.textContent = text
  return div.innerHTML
}

function formatNumber(n: number): string {
  if (n >= 1000000) return (n / 1000000).toFixed(1) + 'M'
  if (n >= 1000) return (n / 1000).toFixed(1) + 'K'
  return n.toString()
}

function shortenPath(path: string): string {
  // Show last 2-3 path segments
  const parts = path.split('/')
  if (parts.length <= 3) return path
  return '.../' + parts.slice(-3).join('/')
}
