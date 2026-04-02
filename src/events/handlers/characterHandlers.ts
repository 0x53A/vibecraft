/**
 * Character Movement Event Handlers
 *
 * Handles Claude character movement in response to tool use events.
 * Moves character to appropriate stations and sets context labels.
 */

import { eventBus } from '../EventBus'
import { soundManager } from '../../audio'
import { getToolContext } from '../../utils/ToolUtils'
import { getStationForTool } from '../../../shared/types'
import type { PreToolUseEvent, PostToolUseEvent, StopEvent, UserPromptSubmitEvent } from '../../../shared/types'

/**
 * Register character movement event handlers
 */
export function registerCharacterHandlers(): void {
  // Move character to station when tool starts
  eventBus.on('pre_tool_use', (event: PreToolUseEvent, ctx) => {
    if (ctx.isHistory || !ctx.session) return

    const station = getStationForTool(event.tool)

    // Move character to station (skip 'center' - those are MCP browser tools)
    if (station !== 'center') {
      const zoneStation = ctx.session.zone.stations.get(station)
      if (zoneStation) {
        ctx.session.claude.moveToPosition(zoneStation.position, station)
        // Play walking sound
        if (ctx.soundEnabled) {
          soundManager.play('walking')
        }
      }
    }

    // Set context text above station
    if (ctx.scene && station !== 'center') {
      const context = getToolContext(event.tool, event.toolInput)
      if (context) {
        ctx.scene.setStationContext(station, context, event.sessionId)
      }

      // Pulse station ring to highlight activity
      ctx.scene.pulseStation(event.sessionId, station)
    }
  })

  // Set idle state when tool completes (if not walking)
  eventBus.on('post_tool_use', (_event: PostToolUseEvent, ctx) => {
    if (ctx.isHistory || !ctx.session) return

    // Only set idle if character isn't walking
    if (ctx.session.claude.state !== 'walking') {
      ctx.session.claude.setState('idle')
    }
  })

  // Move character back to center when stopped
  eventBus.on('stop', (event: StopEvent, ctx) => {
    if (ctx.isHistory || !ctx.session || !ctx.scene) return

    // Move to zone center
    const centerStation = ctx.session.zone.stations.get('center')
    if (centerStation) {
      ctx.session.claude.moveToPosition(centerStation.position, 'center')
    }

    // Clear station context labels
    ctx.scene.clearAllContexts(event.sessionId)
  })

  // Set thinking state when user submits prompt
  eventBus.on('user_prompt_submit', (_event: UserPromptSubmitEvent, ctx) => {
    if (ctx.isHistory || !ctx.session) return
    ctx.session.claude.setState('thinking')
  })
}
