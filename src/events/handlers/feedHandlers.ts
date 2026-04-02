/**
 * Feed/UI Event Handlers
 *
 * Handles thinking indicator visibility in the activity feed.
 */

import { eventBus } from '../EventBus'
import type { PreToolUseEvent, StopEvent } from '../../../shared/types'

/**
 * Register feed-related event handlers
 */
export function registerFeedHandlers(): void {
  // Hide thinking indicator when tool starts
  eventBus.on('pre_tool_use', (_event: PreToolUseEvent, ctx) => {
    if (ctx.isHistory || !ctx.session || !ctx.feedManager) return
    ctx.feedManager.hideThinking(_event.sessionId)
  })

  // Hide thinking indicator on stop
  eventBus.on('stop', (event: StopEvent, ctx) => {
    if (ctx.isHistory || !ctx.feedManager) return
    ctx.feedManager.hideThinking(event.sessionId)
  })

  // NOTE: showThinking for user_prompt_submit is handled in main.ts
  // AFTER feedManager.add() to ensure correct ordering in the feed
}
