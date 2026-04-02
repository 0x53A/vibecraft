/**
 * SessionAPI - Pure API layer for session management
 *
 * All functions are pure HTTP calls with no DOM/state dependencies.
 * UI logic and state updates are handled by the caller (main.ts).
 */

import type { ManagedSession } from '../../shared/types'

export interface McpServerConfig {
  name: string
  command: string
  args?: string[]
}

export interface TentaclesTarget {
  name: string
  targetType: string
  params?: Record<string, unknown>
}

export interface TentaclesConfig {
  enabled: boolean
  targets: TentaclesTarget[]
}

export interface SessionFlags {
  continue?: boolean
  skipPermissions?: boolean
  chrome?: boolean
  tools?: string[]
  mcpServers?: McpServerConfig[]
  tentacles?: TentaclesConfig
  systemPromptMode?: string
  systemPromptText?: string
  memory?: boolean
}

export interface PromptTemplate {
  name: string
  text: string
  createdAt: number
  updatedAt: number
}

export interface CreateSessionResponse {
  ok: boolean
  error?: string
  session?: ManagedSession
}

export interface SimpleResponse {
  ok: boolean
  error?: string
}

export interface ServerInfoResponse {
  ok: boolean
  cwd?: string
  error?: string
}

export interface ResumableSession {
  sessionId: string
  cwd: string
  startedAt: number
  pid: number
}

export interface ResumableSessionsResponse {
  ok: boolean
  sessions?: ResumableSession[]
  error?: string
}

/**
 * Create a SessionAPI instance bound to a specific API URL
 */
export function createSessionAPI(apiUrl: string) {
  return {
    /**
     * Create a new managed session
     */
    async createSession(
      name?: string,
      cwd?: string,
      flags?: SessionFlags,
      resume?: string
    ): Promise<CreateSessionResponse> {
      try {
        const response = await fetch(`${apiUrl}/sessions`, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ name, cwd, flags, resume }),
        })
        return await response.json()
      } catch (e) {
        console.error('Error creating session:', e)
        return { ok: false, error: 'Network error' }
      }
    },

    /**
     * Fetch server info (cwd, etc.)
     */
    async getServerInfo(): Promise<ServerInfoResponse> {
      try {
        const response = await fetch(`${apiUrl}/info`)
        return await response.json()
      } catch (e) {
        console.error('Error fetching server info:', e)
        return { ok: false, error: 'Network error' }
      }
    },

    /**
     * Rename a managed session
     */
    async renameSession(sessionId: string, name: string): Promise<SimpleResponse> {
      try {
        const response = await fetch(`${apiUrl}/sessions/${sessionId}`, {
          method: 'PATCH',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ name }),
        })
        return await response.json()
      } catch (e) {
        console.error('Error renaming session:', e)
        return { ok: false, error: 'Network error' }
      }
    },

    /**
     * Save zone position for a managed session
     */
    async saveZonePosition(
      sessionId: string,
      position: { q: number; r: number }
    ): Promise<SimpleResponse> {
      try {
        const response = await fetch(`${apiUrl}/sessions/${sessionId}`, {
          method: 'PATCH',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ zonePosition: position }),
        })
        return await response.json()
      } catch (e) {
        console.error('Error saving zone position:', e)
        return { ok: false, error: 'Network error' }
      }
    },

    /**
     * Delete a managed session
     */
    async deleteSession(sessionId: string): Promise<SimpleResponse> {
      try {
        const response = await fetch(`${apiUrl}/sessions/${sessionId}`, {
          method: 'DELETE',
        })
        return await response.json()
      } catch (e) {
        console.error('Error deleting session:', e)
        return { ok: false, error: 'Network error' }
      }
    },

    /**
     * Get spawn flags and metadata for a session (used to prepopulate restart UI)
     */
    async getSessionSpawnFlags(sessionId: string): Promise<{
      ok: boolean
      spawnFlags?: SessionFlags
      name?: string
      cwd?: string
      claudeSessionId?: string
      error?: string
    }> {
      try {
        const response = await fetch(`${apiUrl}/sessions/${sessionId}/spawn-flags`)
        return await response.json()
      } catch (e) {
        console.error('Error fetching session spawn flags:', e)
        return { ok: false, error: 'Network error' }
      }
    },

    /**
     * Send a prompt to a managed session
     */
    async sendPrompt(
      sessionId: string,
      prompt: string
    ): Promise<SimpleResponse> {
      try {
        const response = await fetch(`${apiUrl}/sessions/${sessionId}/prompt`, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ prompt }),
        })
        return await response.json()
      } catch (e) {
        console.error('Error sending prompt:', e)
        return { ok: false, error: 'Network error' }
      }
    },

    /**
     * Link a Claude session ID to a managed session
     */
    async linkSession(
      managedId: string,
      claudeSessionId: string
    ): Promise<void> {
      try {
        await fetch(`${apiUrl}/sessions/${managedId}/link`, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ claudeSessionId }),
        })
      } catch (e) {
        console.error('Failed to link session on server:', e)
      }
    },

    /**
     * Trigger a health check / refresh of all sessions
     */
    async refreshSessions(): Promise<void> {
      try {
        await fetch(`${apiUrl}/sessions/refresh`, { method: 'POST' })
      } catch (e) {
        console.error('Error refreshing sessions:', e)
      }
    },

    async getResumableSessions(): Promise<ResumableSessionsResponse> {
      try {
        const response = await fetch(`${apiUrl}/sessions/resumable`)
        return await response.json()
      } catch (e) {
        console.error('Error fetching resumable sessions:', e)
        return { ok: false, error: 'Network error' }
      }
    },

    /**
     * List tentacles targets for a session
     */
    async listTargets(sessionId: string): Promise<{ ok: boolean; targets?: TentaclesTarget[]; error?: string }> {
      try {
        const response = await fetch(`${apiUrl}/sessions/${sessionId}/targets`)
        return await response.json()
      } catch (e) {
        console.error('Error listing targets:', e)
        return { ok: false, error: 'Network error' }
      }
    },

    /**
     * Add a tentacles target to a session
     */
    async addTarget(sessionId: string, target: TentaclesTarget): Promise<SimpleResponse> {
      try {
        const response = await fetch(`${apiUrl}/sessions/${sessionId}/targets`, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify(target),
        })
        return await response.json()
      } catch (e) {
        console.error('Error adding target:', e)
        return { ok: false, error: 'Network error' }
      }
    },

    /**
     * Remove a tentacles target from a session
     */
    async removeTarget(sessionId: string, name: string): Promise<SimpleResponse> {
      try {
        const response = await fetch(`${apiUrl}/sessions/${sessionId}/targets/${name}`, {
          method: 'DELETE',
        })
        return await response.json()
      } catch (e) {
        console.error('Error removing target:', e)
        return { ok: false, error: 'Network error' }
      }
    },

    /**
     * List all prompt templates
     */
    async listTemplates(): Promise<{ ok: boolean; templates?: PromptTemplate[]; error?: string }> {
      try {
        const response = await fetch(`${apiUrl}/templates`)
        return await response.json()
      } catch (e) {
        console.error('Error listing templates:', e)
        return { ok: false, error: 'Network error' }
      }
    },

    /**
     * Save (create or update) a prompt template
     */
    async saveTemplate(name: string, text: string): Promise<{ ok: boolean; template?: PromptTemplate; error?: string }> {
      try {
        const response = await fetch(`${apiUrl}/templates`, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ name, text }),
        })
        return await response.json()
      } catch (e) {
        console.error('Error saving template:', e)
        return { ok: false, error: 'Network error' }
      }
    },

    /**
     * Delete a prompt template by name
     */
    async deleteTemplate(name: string): Promise<SimpleResponse> {
      try {
        const response = await fetch(`${apiUrl}/templates/${encodeURIComponent(name)}`, {
          method: 'DELETE',
        })
        return await response.json()
      } catch (e) {
        console.error('Error deleting template:', e)
        return { ok: false, error: 'Network error' }
      }
    },
  }
}

export type SessionAPI = ReturnType<typeof createSessionAPI>
