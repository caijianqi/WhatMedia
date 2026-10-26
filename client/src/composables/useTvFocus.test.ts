import { describe, expect, it, beforeEach } from 'vitest'
import {
  isElementVisible,
  getActiveModalScope,
  collectFocusables,
} from './useTvFocus'

describe('useTvFocus helper utilities', () => {
  beforeEach(() => {
    document.body.innerHTML = ''
  })

  it('isElementVisible correctly identifies visible and invisible elements', () => {
    const el = document.createElement('button')
    el.textContent = 'Action'
    document.body.appendChild(el)

    // JSDOM mock getBoundingClientRect
    el.getBoundingClientRect = () => ({
      width: 100,
      height: 40,
      top: 10,
      left: 10,
      right: 110,
      bottom: 50,
      x: 10,
      y: 10,
      toJSON: () => {},
    })

    expect(isElementVisible(el)).toBe(true)

    // Hidden by display none
    el.style.display = 'none'
    expect(isElementVisible(el)).toBe(false)
  })

  it('getActiveModalScope discovers modal when present', () => {
    expect(getActiveModalScope()).toBeNull()

    const modal = document.createElement('div')
    modal.className = 'vod-modal-backdrop'
    modal.getBoundingClientRect = () => ({
      width: 800,
      height: 600,
      top: 0,
      left: 0,
      right: 800,
      bottom: 600,
      x: 0,
      y: 0,
      toJSON: () => {},
    })
    document.body.appendChild(modal)

    expect(getActiveModalScope()).toBe(modal)
  })

  it('collectFocusables retrieves buttons and links with accurate rects', () => {
    const btn1 = document.createElement('button')
    btn1.id = 'btn-1'
    btn1.getBoundingClientRect = () => ({
      width: 100,
      height: 50,
      top: 0,
      left: 0,
      right: 100,
      bottom: 50,
      x: 0,
      y: 0,
      toJSON: () => {},
    })

    const btn2 = document.createElement('button')
    btn2.id = 'btn-2'
    btn2.getBoundingClientRect = () => ({
      width: 100,
      height: 50,
      top: 60,
      left: 0,
      right: 100,
      bottom: 110,
      x: 0,
      y: 60,
      toJSON: () => {},
    })

    document.body.appendChild(btn1)
    document.body.appendChild(btn2)

    const { elements, items } = collectFocusables(document.body)
    expect(elements.length).toBe(2)
    expect(items.length).toBe(2)
    expect(items[0]?.id).toBe('btn-1')
    expect(items[1]?.id).toBe('btn-2')
  })
})
