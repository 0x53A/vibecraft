/**
 * Subagent Event Handlers
 *
 * Handles spawning and removing subagent visualizations
 * when Task tools start and complete.
 */

import { eventBus } from '../EventBus'
import type { PreToolUseEvent, PostToolUseEvent } from '../../../shared/types'

/**
 * Register subagent-related event handlers
 */
export function registerSubagentHandlers(): void {
  // Spawn subagent when Task tool starts
  eventBus.on('pre_tool_use', (event: PreToolUseEvent, ctx) => {
    if (ctx.isHistory || !ctx.session) return
    if (event.tool !== 'Task') return

    const description = (event.toolInput as { description?: string }).description
    ctx.session.subagents.spawn(event.toolUseId, description)
    ctx.session.stats.activeSubagents = ctx.session.subagents.count
  })

  // Remove subagent when Task tool completes
  eventBus.on('post_tool_use', (event: PostToolUseEvent, ctx) => {
    if (ctx.isHistory || !ctx.session) return
    if (event.tool !== 'Task') return

    ctx.session.subagents.remove(event.toolUseId)
    ctx.session.stats.activeSubagents = ctx.session.subagents.count
  })
}
