// 本项目修改说明与原始来源见仓库根目录 NOTICE
import type { AccountAuthorizationView } from '../../composables/useAccountAuthorization'
import type { AccountCreateForm } from '../../utils/accountCreate'
import { t } from '@/i18n'
import { accountImportModes, usesAuthorization } from '../../utils/accountCreate'

export function resolveAccountCreatePresentation(input: {
  form: AccountCreateForm
  provider: string | undefined
  authorization: AccountAuthorizationView
  callback: string
  busy: boolean
  reauthorizing: boolean
}) {
  const configuring = !input.reauthorizing && input.form.step === 'settings'
  const authorize = usesAuthorization(input.form)
  return {
    configuring,
    isBatch: false,
    label: 'Claude Code',
    modeOptions: accountImportModes(input.provider),
    modal: {
      title: configuring
        ? t('accounts.onboarding.accountSettings')
        : input.reauthorizing
          ? t('accounts.onboarding.reauthorizeAccount')
          : t('accounts.filters.importAccount'),
      description: t('accounts.claude.description'),
      type: 'info' as const,
      width: '42rem',
    },
    oauth: {
      callbackLabel: t('accounts.claude.callbackLabel'),
      callbackPlaceholder: t('accounts.claude.callbackPlaceholder'),
      description: t('accounts.claude.authorizationHelp'),
    },
    canSubmit: !input.busy && (authorize
      ? Boolean(input.authorization.flow && input.callback.trim() && input.authorization.status !== 'expired')
      : Boolean(input.form.setupToken.token.trim() && input.form.name.trim())),
    submitLabel: authorize
      ? t('accounts.onboarding.completeImport')
      : t('accounts.filters.importAccount'),
  }
}
