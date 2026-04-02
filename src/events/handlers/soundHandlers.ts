/**
 * Sound Event Handlers
 *
 * Registers sound effects for various events.
 * These run via the EventBus, decoupled from main event handling.
 * Includes spatial audio positioning based on session/zone.
 */

import { eventBus } from '../EventBus'
import { soundManager, type SoundPlayOptions } from '../../audio'
import type { PreToolUseEvent, PostToolUseEvent, BashToolInput } from '../../../shared/types'

/**
 * Check if a Bash command is a git commit
 */
function isGitCommit(command: string): boolean {
  // Match: git commit, git commit -m, git commit --amend, etc.
  // But not: git commit-tree, commented out commands, etc.
  return /\bgit\s+commit\b/.test(command)
}

/**
 * Get spatial options from context (for positional audio)
 */
function getSpatialOptions(ctx: { session: { id: string } | null }): SoundPlayOptions | undefined {
  if (ctx.session?.id) {
    return { zoneId: ctx.session.id }
  }
  return undefined
}

/**
 * Register all sound-related event handlers
 */
export function registerSoundHandlers(): void {
  // Tool start sounds (with special handling for git commit)
  eventBus.on('pre_tool_use', (event: PreToolUseEvent, ctx) => {
    if (ctx.isHistory || !ctx.soundEnabled) return
    const spatial = getSpatialOptions(ctx)

    // Special sound for git commit (global, no spatial)
    if (event.tool === 'Bash') {
      const input = event.toolInput as unknown as BashToolInput
      if (input.command && isGitCommit(input.command)) {
        soundManager.play('git_commit')  // Global sound, no spatial
        return  // Skip normal bash sound
      }
    }

    soundManager.playTool(event.tool, spatial)
  })

  // Subagent spawn sound (Task tool start)
  eventBus.on('pre_tool_use', (event: PreToolUseEvent, ctx) => {
    if (ctx.isHistory || !ctx.soundEnabled) return
    if (event.tool === 'Task') {
      const spatial = getSpatialOptions(ctx)
      soundManager.play('spawn', spatial)
    }
  })

  // Tool completion sounds (success/error)
  eventBus.on('post_tool_use', (event: PostToolUseEvent, ctx) => {
    if (ctx.isHistory || !ctx.soundEnabled) return
    const spatial = getSpatialOptions(ctx)
    soundManager.playResult(event.success, spatial)
  })

  // Subagent despawn sound
  eventBus.on('post_tool_use', (event: PostToolUseEvent, ctx) => {
    if (ctx.isHistory || !ctx.soundEnabled) return
    if (event.tool === 'Task') {
      const spatial = getSpatialOptions(ctx)
      soundManager.play('despawn', spatial)
    }
  })

  // Stop/completion sound
  eventBus.on('stop', (_event, ctx) => {
    if (ctx.isHistory || !ctx.soundEnabled) return
    const spatial = getSpatialOptions(ctx)
    soundManager.play('stop', spatial)
  })

  // Prompt received sound
  eventBus.on('user_prompt_submit', (_event, ctx) => {
    if (ctx.isHistory || !ctx.soundEnabled) return
    const spatial = getSpatialOptions(ctx)
    soundManager.play('prompt', spatial)
  })

  // Notification sound (global, no spatial)
  eventBus.on('notification', (_event, ctx) => {
    if (ctx.isHistory || !ctx.soundEnabled) return
    soundManager.play('notification')  // Global sound, no spatial
  })
}
