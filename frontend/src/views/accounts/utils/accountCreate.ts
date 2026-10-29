// 本项目修改说明与原始来源见仓库根目录 NOTICE
import type { AccountModelAccess } from '@/api'
import { t } from '@/i18n'
import { accountModelAccessError } from './modelAccess'
import { parseAccountSchedulingForm } from './schedulingForm'

export interface AccountCreateSource { kind: 'provider', id: string }
export type AccountImportMode = 'oauth' | 'setup_token'

export interface AccountCreateForm {
  name: string
  setupToken: { input: 'authorize' | 'paste', token: string, expiresAt: string }
  source: AccountCreateSource | null
  notes: string
  enabled: boolean
  concurrencyLimit: string
  weight: string
  modelAccess?: AccountModelAccess
  groupIds: string[]
  step: 'settings' | 'import'
  mode: AccountImportMode
  proxyMode: string
  proxyId: string
}

export function emptyAccountCreateForm(): AccountCreateForm {
  return {
    name: '',
    setupToken: { input: 'authorize', token: '', expiresAt: '' },
    source: { kind: 'provider', id: 'claude' },
    notes: '',
    enabled: true,
    concurrencyLimit: '',
    weight: '1',
    groupIds: [],
    step: 'settings',
    mode: 'oauth',
    proxyMode: 'proxy',
    proxyId: '',
  }
}

export function accountCreateProvider(form: AccountCreateForm) {
  return form.source?.kind === 'provider'
    ? form.source.id
    : undefined
}

export function accountCreateSourceKey(form: AccountCreateForm) {
  return form.source?.kind === 'provider'
    ? `provider:${form.source.id}`
    : form.source?.kind ?? ''
}

export function accountProxyError(form: AccountCreateForm): string | undefined {
  if (form.proxyMode !== 'proxy' || !form.proxyId.trim())
    return t('accounts.batchEdit.proxyRequired')
  return undefined
}

export function accountImportSettings(form: AccountCreateForm) {
  const scheduling = parseAccountSchedulingForm(form.concurrencyLimit, form.weight)
  if (!scheduling.valid)
    throw new Error(scheduling.message)
  const modelError = accountModelAccessError(form.modelAccess)
  if (modelError)
    throw new Error(modelError)
  return {
    modelAccess: form.modelAccess
      ? {
          ...form.modelAccess,
          models: [...form.modelAccess.models],
        }
      : undefined,
    enabled: form.enabled,
    ...scheduling.values,
    groupIds: [
      ...new Set(form.groupIds),
    ],
    notes: form.notes.trim() || undefined,
  }
}

export function accountImportModes(provider: string | undefined): { label: string, value: AccountImportMode }[] {
  return provider === 'claude'
    ? [
        { label: 'OAuth', value: 'oauth' },
        {
          label: t('accounts.claude.setupToken'),
          value: 'setup_token',
        },
      ]
    : []
}

export function usesAuthorization(form: AccountCreateForm): boolean {
  return form.mode === 'oauth' || (form.mode === 'setup_token' && form.setupToken.input === 'authorize')
}
