/**
 * Permission Prompt - Inline tool permission request in activity feed
 *
 * Displays permission prompts as feed items when Claude sessions run without
 * --dangerously-skip-permissions and need user approval for tools.
 */

import { soundManager } from '../audio'
import type { FeedManager } from './FeedManager'
import type { WorkshopScene } from '../scene/WorkshopScene'
import type { AttentionSystem } from '../systems/AttentionSystem'
import type { ManagedSession } from '../../shared/types'

// ============================================================================
// Types
// ============================================================================

export interface PermissionOption {
  number: string
  label: string
}

export interface PermissionData {
  sessionId: string
  permissionId: string
  tool: string
  context: string
  options: PermissionOption[]
}

export interface PermissionModalContext {
  scene: WorkshopScene | null
  soundEnabled: boolean
  apiUrl: string
  attentionSystem: AttentionSystem | null
  getManagedSessions: () => ManagedSession[]
  feedManager: FeedManager | null
  getSessionColor: (managedId: string) => number | undefined
  /** Convert managed session ID to Claude session ID (used for feed filtering) */
  getClaudeSessionId: (managedId: string) => string | undefined
}

// ============================================================================
// State
// ============================================================================

let currentPermission: PermissionData | null = null
let context: PermissionModalContext | null = null

// ============================================================================
// Public API
// ============================================================================

/**
 * Initialize the permission system with dependencies
 */
export function setupPermissionModal(ctx: PermissionModalContext): void {
  context = ctx

  // Keyboard shortcuts - press the number key to select that option
  document.addEventListener('keydown', (e) => {
    if (!currentPermission) return

    // Don't intercept if user is typing in an input
    const target = e.target as HTMLElement
    if (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA') return

    // Number keys 1-9 to select options
    if (/^[1-9]$/.test(e.key)) {
      const option = currentPermission.options.find(o => o.number === e.key)
      if (option) {
        e.preventDefault()
        sendPermissionResponse(currentPermission.permissionId, option.number)
      }
    }
  })
}

/**
 * Show a permission prompt inline in the activity feed
 */
export function showPermissionModal(
  sessionId: string,
  permissionId: string,
  tool: string,
  permContext: string,
  options: PermissionOption[]
): void {
  if (!context?.feedManager) return

  currentPermission = { sessionId, permissionId, tool, context: permContext, options }

  const sessionColor = context.getSessionColor(sessionId)
  // Feed uses Claude session IDs for filtering, not managed session IDs
  const feedSessionId = context.getClaudeSessionId(sessionId) ?? sessionId

  // Add inline permission prompt to the feed
  context.feedManager.showPermission(
    feedSessionId,
    tool,
    permContext,
    options,
    (response) => sendPermissionResponse(permissionId, response),
    sessionColor
  )

  // Set attention on the session's zone
  const managed = context.getManagedSessions().find(s => s.id === sessionId)
  if (managed?.claudeSessionId && context.scene) {
    context.scene.setZoneAttention(managed.claudeSessionId, 'question')
    context.scene.setZoneStatus(managed.claudeSessionId, 'attention')
  }

  // Add to attention queue
  context.attentionSystem?.add(sessionId)

  // Play notification sound
  if (context.soundEnabled) {
    soundManager.play('notification')
  }
}

/**
 * Hide/resolve the permission prompt in the feed
 */
export function hidePermissionModal(): void {
  if (!currentPermission || !context?.feedManager) {
    currentPermission = null
    return
  }

  // Remove from feed
  const feedSessionId = context.getClaudeSessionId(currentPermission.sessionId) ?? currentPermission.sessionId
  context.feedManager.hidePermission(feedSessionId)

  // Clear attention
  const managed = context.getManagedSessions().find(s => s.id === currentPermission!.sessionId)
  if (managed?.claudeSessionId && context.scene) {
    context.scene.clearZoneAttention(managed.claudeSessionId)
    context.scene.setZoneStatus(managed.claudeSessionId, 'working')
  }
  context.attentionSystem?.remove(currentPermission.sessionId)

  currentPermission = null
}

/**
 * Check if a permission prompt is currently active
 */
export function isPermissionModalVisible(): boolean {
  return currentPermission !== null
}

// ============================================================================
// Internal
// ============================================================================

async function sendPermissionResponse(permissionId: string, response: string): Promise<void> {
  if (!currentPermission || !context) return

  try {
    await fetch(`${context.apiUrl}/sessions/${currentPermission.sessionId}/permission`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ permissionId, response }),
    })
  } catch (e) {
    console.error('Failed to send permission response:', e)
  }

  hidePermissionModal()
}
