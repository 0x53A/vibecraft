/**
 * Zone Notification Event Handlers
 *
 * Shows floating notifications above zones when tools complete.
 * Uses ZoneNotifications system for tool-specific styling.
 */

import { eventBus } from '../EventBus'
import {
  formatFileChange,
  formatCommandResult,
  formatSearchResult,
} from '../../scene/ZoneNotifications'
import type { PostToolUseEvent } from '../../../shared/types'
import { getStationForTool } from '../../../shared/types'

/**
 * Register notification-related event handlers
 */
export function registerNotificationHandlers(): void {
  // Tool completion notifications
  eventBus.on('post_tool_use', (event: PostToolUseEvent, ctx) => {
    if (ctx.isHistory || !event.success || !ctx.scene) return

    const input = event.toolInput as Record<string, unknown>
    let notificationText: string | null = null

    switch (event.tool) {
      case 'Edit': {
        const filePath = input.file_path as string | undefined
        if (filePath) {
          const fileName = filePath.split('/').pop() || filePath
          const oldStr = input.old_string as string | undefined
          const newStr = input.new_string as string | undefined
          if (oldStr && newStr) {
            const oldLines = (oldStr.match(/\n/g) || []).length + 1
            const newLines = (newStr.match(/\n/g) || []).length + 1
            const added = Math.max(0, newLines - oldLines)
            const removed = Math.max(0, oldLines - newLines)
            notificationText = formatFileChange(fileName, { added, removed })
          } else {
            notificationText = fileName
          }
        }
        break
      }
      case 'Write': {
        const filePath = input.file_path as string | undefined
        if (filePath) {
          const fileName = filePath.split('/').pop() || filePath
          const content = input.content as string | undefined
          if (content) {
            const lines = (content.match(/\n/g) || []).length + 1
            notificationText = formatFileChange(fileName, { lines })
          } else {
            notificationText = fileName
          }
        }
        break
      }
      case 'Read': {
        const filePath = input.file_path as string | undefined
        if (filePath) {
          notificationText = filePath.split('/').pop() || filePath
        }
        break
      }
      case 'Bash': {
        const command = input.command as string | undefined
        if (command) {
          notificationText = formatCommandResult(command)
        }
        break
      }
      case 'Grep':
      case 'Glob': {
        const pattern = input.pattern as string | undefined
        if (pattern) {
          notificationText = formatSearchResult(pattern)
        }
        break
      }
      case 'WebFetch':
      case 'WebSearch': {
        const url = input.url as string | undefined
        const query = input.query as string | undefined
        if (url) {
          // Extract domain from URL
          try {
            const domain = new URL(url).hostname
            notificationText = domain
          } catch {
            notificationText = url.slice(0, 30)
          }
        } else if (query) {
          notificationText = formatSearchResult(query)
        }
        break
      }
      case 'Task': {
        const description = input.description as string | undefined
        if (description) {
          notificationText = description.slice(0, 25)
        }
        break
      }
      case 'TodoWrite': {
        const todos = input.todos as Array<{ content?: string }> | undefined
        if (todos && todos.length > 0) {
          notificationText = `${todos.length} items`
        }
        break
      }
    }

    // Show notification using zone notifications system
    if (notificationText) {
      ctx.scene.zoneNotifications.showForTool(event.sessionId, event.tool, notificationText)

      // Also update station panels with tool history
      const station = getStationForTool(event.tool)
      if (station !== 'center') {
        ctx.scene.stationPanels.addToolUse(event.sessionId, station, {
          text: notificationText,
          success: event.success,
        })
      }
    }
  })
}
