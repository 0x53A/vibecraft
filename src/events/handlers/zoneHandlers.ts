/**
 * Zone Status Event Handlers
 *
 * Handles zone status updates (working, attention, etc.)
 * and attention states for questions and completion.
 */

import { eventBus } from '../EventBus'
import type { StopEvent, UserPromptSubmitEvent } from '../../../shared/types'

/**
 * Register zone status event handlers
 */
export function registerZoneHandlers(): void {
  // Set attention state when Claude stops (finished work)
  eventBus.on('stop', (event: StopEvent, ctx) => {
    if (ctx.isHistory || !ctx.session || !ctx.scene) return

    // Set finished attention - agent completed its work
    ctx.scene.setZoneAttention(event.sessionId, 'finished')
    ctx.scene.setZoneStatus(event.sessionId, 'attention')  // Red glow
  })

  // Clear attention and set working when user submits prompt
  eventBus.on('user_prompt_submit', (event: UserPromptSubmitEvent, ctx) => {
    if (ctx.isHistory || !ctx.session || !ctx.scene) return

    // Clear attention - user is now engaged
    ctx.scene.clearZoneAttention(event.sessionId)
    ctx.scene.setZoneStatus(event.sessionId, 'working')  // Cyan glow
  })
}
