import { beforeEach, describe, expect, it, vi } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'
import { mount } from '@vue/test-utils'
import LoginView from './LoginView.vue'
import { useSessionStore } from '../stores/session'

const replaceMock = vi.fn()

vi.mock('vue-router', () => ({
  useRouter: () => ({ replace: replaceMock, push: vi.fn() }),
  useRoute: () => ({ name: 'login', fullPath: '/login', query: {} }),
}))

vi.mock('../api/client', () => ({
  login: vi.fn(),
  setup: vi.fn(),
  whoami: vi.fn(),
  health: vi.fn().mockResolvedValue({ status: 'ok', version: '0.1.0' }),
}))

describe('LoginView.vue 密码长度限制与表单校验', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    replaceMock.mockReset()
  })

  it('密码输入框 minlength 为 6', () => {
    const wrapper = mount(LoginView)
    const pwdInput = wrapper.find('input[type="password"]')
    expect(pwdInput.exists()).toBe(true)
    expect(pwdInput.attributes('minlength')).toBe('6')
  })

  it('密码少于 6 个字符时禁用提交按钮，达到 6 个字符时启用', async () => {
    const wrapper = mount(LoginView)
    const userInput = wrapper.find('input[type="text"]')
    const pwdInput = wrapper.find('input[type="password"]')
    const submitBtn = wrapper.find('button[type="submit"]')

    // 输入用户名，但未填密码
    await userInput.setValue('admin')
    expect((submitBtn.element as HTMLButtonElement).disabled).toBe(true)

    // 密码 5 位
    await pwdInput.setValue('12345')
    expect((submitBtn.element as HTMLButtonElement).disabled).toBe(true)

    // 密码达到 6 位
    await pwdInput.setValue('123456')
    expect((submitBtn.element as HTMLButtonElement).disabled).toBe(false)
  })

  it('提交成功后跳转首页', async () => {
    const wrapper = mount(LoginView)
    const session = useSessionStore()
    vi.spyOn(session, 'login').mockResolvedValue(true)

    await wrapper.find('input[type="text"]').setValue('admin')
    await wrapper.find('input[type="password"]').setValue('123456')

    await wrapper.find('form').trigger('submit.prevent')
    expect(session.login).toHaveBeenCalledWith('admin', '123456')
    expect(replaceMock).toHaveBeenCalledWith({ name: 'home' })
  })
})
