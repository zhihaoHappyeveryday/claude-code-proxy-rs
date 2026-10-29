// 本项目修改说明与原始来源见仓库根目录 NOTICE
import type { Component } from 'vue'
import { Key, LinkAlt } from '@boxicons/vue'
import { Fingerprint, Plug, Sparkles } from '@lucide/vue'
import { t } from '@/i18n'

const PROVIDER_DISPLAY_NAMES = {
  claude: 'Claude',
} as const

export const PROVIDER_IDS = Object.keys(PROVIDER_DISPLAY_NAMES) as Array<keyof typeof PROVIDER_DISPLAY_NAMES>

type SupportedProvider = keyof typeof PROVIDER_DISPLAY_NAMES

export function isSupportedProvider(value: unknown): value is SupportedProvider {
  return typeof value === 'string' && Object.hasOwn(PROVIDER_DISPLAY_NAMES, value)
}

export function formatProviderLabel(value?: string | null, fallback = '—') {
  const normalized = value?.trim()
  const provider = normalized?.toLowerCase()
  return isSupportedProvider(provider)
    ? PROVIDER_DISPLAY_NAMES[provider]
    : (normalized || fallback)
}

export function providerIcon(value: string): Component {
  switch (value.trim().toLowerCase()) {
    case 'claude': return Sparkles
    default: return Plug
  }
}

export function formatAuthenticationLabel(value?: string | null) {
  const normalized = value?.trim()
  switch (normalized?.toLowerCase()) {
    case 'oauth': return 'OAuth'
    case 'setup_token': return 'Setup Token'
    case 'api_key': return t('common.fields.apiKey')
    default: return normalized
      ? t('common.providers.customAuth', { normalized })
      : t('common.providers.unknownAuth')
  }
}

export function authenticationIcon(value?: string | null): Component | undefined {
  switch (value?.trim().toLowerCase()) {
    case 'oauth': return LinkAlt
    case 'api_key': return Key
    case undefined:
    case '': return undefined
    default: return Fingerprint
  }
}
