/**
 * Template Editor Modal — composition tool for system prompt templates.
 *
 * Shows Claude Code's default system prompt sections (readonly, toggleable)
 * with insertion points between them. Flattens to raw text on save.
 */

import { SYSTEM_PROMPT_SECTIONS, type SystemPromptSection } from '../data/systemPromptSections'
import type { SessionAPI } from '../api/SessionAPI'
import type { PromptTemplate } from '../api/SessionAPI'

interface TemplateEditorState {
  sessionAPI: SessionAPI
  modal: HTMLElement
  nameInput: HTMLInputElement
  sectionsContainer: HTMLElement
  saveBtn: HTMLButtonElement
  deleteBtn: HTMLButtonElement
  cancelBtn: HTMLButtonElement
  onSaved?: (templates: PromptTemplate[]) => void
  editingName?: string // set when editing an existing template
}

let editorState: TemplateEditorState | null = null

export function setupTemplateEditor(
  sessionAPI: SessionAPI,
  onSaved?: (templates: PromptTemplate[]) => void
): void {
  const modal = document.getElementById('template-editor-modal')
  const nameInput = document.getElementById('template-name-input') as HTMLInputElement
  const sectionsContainer = document.getElementById('template-sections-container')
  const saveBtn = document.getElementById('template-editor-save') as HTMLButtonElement
  const deleteBtn = document.getElementById('template-editor-delete') as HTMLButtonElement
  const cancelBtn = document.getElementById('template-editor-cancel') as HTMLButtonElement

  if (!modal || !nameInput || !sectionsContainer || !saveBtn || !deleteBtn || !cancelBtn) return

  editorState = {
    sessionAPI,
    modal,
    nameInput,
    sectionsContainer,
    saveBtn,
    deleteBtn,
    cancelBtn,
    onSaved,
  }

  // Build the sections UI once
  buildSectionsUI(sectionsContainer, SYSTEM_PROMPT_SECTIONS)

  // Wire up buttons
  cancelBtn.addEventListener('click', closeTemplateEditor)
  saveBtn.addEventListener('click', handleSave)
  deleteBtn.addEventListener('click', handleDelete)

  // Close on backdrop click
  modal.addEventListener('click', (e) => {
    if (e.target === modal) closeTemplateEditor()
  })

  // Close on Escape
  modal.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') {
      e.stopPropagation()
      closeTemplateEditor()
    }
  })
}

function buildSectionsUI(container: HTMLElement, sections: SystemPromptSection[]): void {
  container.innerHTML = ''

  // Insertion point before first section
  container.appendChild(createInsertionArea('before-' + sections[0].id))

  for (let i = 0; i < sections.length; i++) {
    const section = sections[i]

    const block = document.createElement('div')
    block.className = 'template-section-block'

    // Header with checkbox
    const header = document.createElement('label')
    header.className = 'template-section-header'

    const checkbox = document.createElement('input')
    checkbox.type = 'checkbox'
    checkbox.checked = true
    checkbox.dataset.sectionId = section.id

    const title = document.createElement('span')
    title.className = 'template-section-title'
    title.textContent = section.title

    const toggleBtn = document.createElement('button')
    toggleBtn.type = 'button'
    toggleBtn.className = 'template-section-toggle'
    toggleBtn.textContent = '▶'
    toggleBtn.addEventListener('click', (e) => {
      e.preventDefault()
      e.stopPropagation()
      const pre = block.querySelector('.template-section-preview') as HTMLElement
      if (pre) {
        const expanded = pre.style.display !== 'none'
        pre.style.display = expanded ? 'none' : 'block'
        toggleBtn.textContent = expanded ? '▶' : '▼'
      }
    })

    header.appendChild(checkbox)
    header.appendChild(title)
    header.appendChild(toggleBtn)
    block.appendChild(header)

    // Readonly preview (collapsed by default)
    const preview = document.createElement('pre')
    preview.className = 'template-section-preview'
    preview.textContent = section.text
    preview.style.display = 'none'
    block.appendChild(preview)

    container.appendChild(block)

    // Insertion point after each section
    container.appendChild(createInsertionArea('after-' + section.id))
  }
}

function createInsertionArea(id: string): HTMLElement {
  const wrapper = document.createElement('div')
  wrapper.className = 'template-insertion-wrapper'

  const textarea = document.createElement('textarea')
  textarea.className = 'template-insertion-area'
  textarea.dataset.insertionId = id
  textarea.placeholder = '+ Insert custom text here...'
  textarea.rows = 1

  // Auto-expand on input
  textarea.addEventListener('input', () => {
    textarea.rows = Math.max(1, textarea.value.split('\n').length)
    if (!textarea.value) textarea.rows = 1
  })

  wrapper.appendChild(textarea)
  return wrapper
}

export function openTemplateEditor(existingTemplate?: PromptTemplate): void {
  if (!editorState) return
  const { modal, nameInput, sectionsContainer, deleteBtn } = editorState

  // Reset UI
  nameInput.value = existingTemplate?.name ?? ''
  editorState.editingName = existingTemplate?.name

  // Reset all checkboxes to checked
  sectionsContainer.querySelectorAll<HTMLInputElement>('input[type="checkbox"]').forEach((cb) => {
    cb.checked = true
  })

  // Clear all insertion areas
  sectionsContainer.querySelectorAll<HTMLTextAreaElement>('.template-insertion-area').forEach((ta) => {
    ta.value = ''
    ta.rows = 1
  })

  // Collapse all previews
  sectionsContainer.querySelectorAll<HTMLElement>('.template-section-preview').forEach((pre) => {
    pre.style.display = 'none'
  })
  sectionsContainer.querySelectorAll<HTMLButtonElement>('.template-section-toggle').forEach((btn) => {
    btn.textContent = '▶'
  })

  // If editing existing template, try to parse it back into sections
  if (existingTemplate) {
    parseTemplateIntoEditor(existingTemplate.text, sectionsContainer)
  }

  // Show/hide delete button
  deleteBtn.style.display = existingTemplate ? '' : 'none'

  modal.classList.add('visible')
  nameInput.focus()
}

/**
 * Best-effort parsing of a template's raw text back into section toggles and insertions.
 * If the text contains a known section verbatim, we keep the checkbox checked.
 * Any text between sections is placed in the corresponding insertion area.
 * If the template doesn't match the section pattern, put everything in the first insertion area
 * and uncheck all sections.
 */
function parseTemplateIntoEditor(text: string, container: HTMLElement): void {
  // Try to find each section in order
  const sections = SYSTEM_PROMPT_SECTIONS
  let remaining = text
  let foundAny = false
  let lastInsertionId = 'before-' + sections[0].id

  for (let i = 0; i < sections.length; i++) {
    const section = sections[i]
    const idx = remaining.indexOf(section.text)

    if (idx >= 0) {
      foundAny = true

      // Text before this section goes into the insertion area
      const before = remaining.substring(0, idx).trim()
      if (before) {
        const ta = container.querySelector<HTMLTextAreaElement>(
          `[data-insertion-id="${lastInsertionId}"]`
        )
        if (ta) {
          ta.value = before
          ta.rows = Math.max(1, before.split('\n').length)
        }
      }

      remaining = remaining.substring(idx + section.text.length)
      lastInsertionId = 'after-' + section.id
    } else {
      // Section not found — uncheck it
      const cb = container.querySelector<HTMLInputElement>(`[data-section-id="${section.id}"]`)
      if (cb) cb.checked = false
    }
  }

  // Any remaining text goes into the last insertion area
  const trailing = remaining.trim()
  if (trailing) {
    const ta = container.querySelector<HTMLTextAreaElement>(
      `[data-insertion-id="${lastInsertionId}"]`
    )
    if (ta) {
      ta.value = trailing
      ta.rows = Math.max(1, trailing.split('\n').length)
    }
  }

  // If nothing matched at all, dump everything in first insertion and uncheck all
  if (!foundAny) {
    container.querySelectorAll<HTMLInputElement>('input[type="checkbox"]').forEach((cb) => {
      cb.checked = false
    })
    const firstTa = container.querySelector<HTMLTextAreaElement>('.template-insertion-area')
    if (firstTa) {
      firstTa.value = text
      firstTa.rows = Math.max(1, text.split('\n').length)
    }
  }
}

function flattenTemplate(): string {
  if (!editorState) return ''
  const { sectionsContainer } = editorState
  const parts: string[] = []

  const insertionAreas = sectionsContainer.querySelectorAll<HTMLTextAreaElement>('.template-insertion-area')
  const sectionBlocks = sectionsContainer.querySelectorAll<HTMLElement>('.template-section-block')

  // Interleave: insertion[0], section[0], insertion[1], section[1], ...
  for (let i = 0; i < sectionBlocks.length; i++) {
    // Insertion before section i
    const insertionBefore = insertionAreas[i]
    if (insertionBefore?.value.trim()) {
      parts.push(insertionBefore.value.trim())
    }

    // Section i (if checked)
    const cb = sectionBlocks[i].querySelector<HTMLInputElement>('input[type="checkbox"]')
    if (cb?.checked) {
      const sectionId = cb.dataset.sectionId
      const section = SYSTEM_PROMPT_SECTIONS.find((s) => s.id === sectionId)
      if (section) parts.push(section.text)
    }
  }

  // Final insertion area (after last section)
  const lastInsertion = insertionAreas[insertionAreas.length - 1]
  if (lastInsertion?.value.trim()) {
    parts.push(lastInsertion.value.trim())
  }

  return parts.join('\n\n')
}

async function handleSave(): Promise<void> {
  if (!editorState) return
  const { sessionAPI, nameInput, onSaved } = editorState

  const name = nameInput.value.trim()
  if (!name) {
    nameInput.focus()
    nameInput.style.borderColor = '#f87171'
    setTimeout(() => { nameInput.style.borderColor = '' }, 1500)
    return
  }

  const text = flattenTemplate()
  const result = await sessionAPI.saveTemplate(name, text)
  if (result.ok) {
    closeTemplateEditor()
    // Refresh template list
    const listResult = await sessionAPI.listTemplates()
    if (listResult.ok && onSaved) {
      onSaved(listResult.templates ?? [])
    }
  }
}

async function handleDelete(): Promise<void> {
  if (!editorState?.editingName) return
  const { sessionAPI, editingName, onSaved } = editorState

  const result = await sessionAPI.deleteTemplate(editingName)
  if (result.ok) {
    closeTemplateEditor()
    const listResult = await sessionAPI.listTemplates()
    if (listResult.ok && onSaved) {
      onSaved(listResult.templates ?? [])
    }
  }
}

export function closeTemplateEditor(): void {
  if (!editorState) return
  editorState.modal.classList.remove('visible')
}
