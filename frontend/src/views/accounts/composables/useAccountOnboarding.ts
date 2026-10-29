// 本项目修改说明与原始来源见仓库根目录 NOTICE
import type { Account } from '@/api'
import { computed, ref, shallowRef, watch } from 'vue'
import { importAccounts } from '@/api'
import { useAsyncAction } from '@/composables/useAsyncAction'
import { t } from '@/i18n'
import { notification as ZNotification } from '@/utils/notification'
import { errorMessage } from '@/utils/operation'
import { formatProviderLabel, isSupportedProvider } from '@/utils/providers'
import { accountCreateProvider, accountCreateSourceKey, accountImportModes, accountImportSettings, accountProxyError, emptyAccountCreateForm, usesAuthorization } from '../utils/accountCreate'
import { useAccountAuthorization } from './useAccountAuthorization'

export function useAccountOnboarding(options: {
  reload: () => Promise<unknown>
}) {
  const createModalOpen = shallowRef(false)
  const reauthorizingAccount = shallowRef<Account | null>(null)
  const creatingAccountAction = useAsyncAction()
  const createForm = ref(emptyAccountCreateForm())
  const authorization = useAccountAuthorization(() => finishCreate(reauthorizingAccount.value
    ? t('accounts.onboarding.reauthorized')
    : t('accounts.onboarding.accountAdded')))
  const showCreateModal = computed({
    get: () => createModalOpen.value,
    set: (value: boolean) => {
      createModalOpen.value = value
      if (!value) {
        authorization.reset()
        createForm.value.setupToken.token = ''
      }
    },
  })

  function clearCreate() {
    authorization.reset()
    reauthorizingAccount.value = null
    createForm.value = emptyAccountCreateForm()
  }

  function requireImportProvider(provider: string | undefined): string {
    if (!provider)
      throw new Error(t('accounts.onboarding.providerRequired'))
    if (!isSupportedProvider(provider))
      throw new Error(t('accounts.onboarding.accountPlatformIsNotSupported'))
    return provider
  }

  async function handleCreate() {
    if (usesAuthorization(createForm.value)) {
      await authorization.complete()
      return
    }
    await creatingAccountAction.run(async () => {
      const form = createForm.value
      if (form.mode !== 'setup_token')
        throw new Error(t('accounts.claude.onlyMethods'))
      const proxyError = accountProxyError(form)
      if (proxyError)
        throw new Error(proxyError)
      await importAccounts({
        provider: requireImportProvider(accountCreateProvider(form)),
        data: {
          name: form.name.trim(),
          authentication_kind: 'setup_token',
          access_token: form.setupToken.token.trim(),
          expires_at: form.setupToken.expiresAt
            ? new Date(form.setupToken.expiresAt).toISOString()
            : null,
        },
        settings: accountImportSettings(form),
        outboundProxyId: form.proxyId.trim(),
      })
      await finishCreate(t('accounts.onboarding.accountAdded'))
      clearCreate()
    })
  }

  async function handleAuthorizeOAuth() {
    if (authorization.busy.value)
      return
    try {
      const form = createForm.value
      const provider = accountCreateProvider(form)
      if (!isSupportedProvider(provider))
        throw new Error(t('accounts.onboarding.accountPlatformIsNotSupported'))
      const account = reauthorizingAccount.value
      const proxyError = account
        ? undefined
        : accountProxyError(form)
      if (proxyError)
        throw new Error(proxyError)
      await authorization.start({
        start: {
          provider,
          name: account?.name || form.name.trim() || formatProviderLabel(provider),
          authorizationMethod: form.mode === 'setup_token'
            ? 'setup_token'
            : 'oauth',
          accountId: account?.id,
          outboundProxyId: account
            ? undefined
            : form.proxyMode === 'proxy'
              ? form.proxyId.trim()
              : undefined,
        },
        settings: account
          ? undefined
          : accountImportSettings(form),
      })
    }
    catch (cause) {
      ZNotification.error({
        message: errorMessage(cause, t('accounts.onboarding.failedToStartAuthorization')),
      })
    }
  }

  function openCreateAccount() {
    authorization.reset()
    reauthorizingAccount.value = null
    createForm.value = emptyAccountCreateForm()
    showCreateModal.value = true
  }

  function openReauthorizeAccount(account: Account) {
    if (isSupportedProvider(account.provider) && !['oauth', 'setup_token'].includes(account.authenticationKind))
      return
    authorization.reset()
    reauthorizingAccount.value = account
    createForm.value = {
      ...emptyAccountCreateForm(),
      source: { kind: 'provider', id: account.provider },
      step: 'import',
      mode: account.authenticationKind === 'setup_token'
        ? 'setup_token'
        : 'oauth',
    }
    showCreateModal.value = true
    void handleAuthorizeOAuth()
  }

  async function finishCreate(message: string) {
    showCreateModal.value = false
    ZNotification.success({ message })
    await options.reload()
  }

  watch(() => accountCreateSourceKey(createForm.value), () => {
    createForm.value = {
      ...createForm.value,
      mode: reauthorizingAccount.value?.authenticationKind === 'setup_token'
        ? 'setup_token'
        : accountImportModes(accountCreateProvider(createForm.value))[0]?.value ?? 'oauth',
      setupToken: { input: 'authorize', token: '', expiresAt: '' },
    }
  }, { flush: 'sync' })

  watch([
    () => accountCreateSourceKey(createForm.value),
    () => createForm.value.mode,
    () => createForm.value.setupToken.input,
    () => createForm.value.step,
    () => createForm.value.proxyMode,
    () => createForm.value.proxyMode === 'proxy'
      ? createForm.value.proxyId.trim()
      : '',
  ], () => {
    authorization.reset()
  }, { flush: 'sync' })

  return {
    showCreateModal,
    reauthorizingAccount,
    creatingAccount: creatingAccountAction.loading,
    authorizingOAuth: authorization.busy,
    authorization: authorization.view,
    authorizationCallback: authorization.callback,
    createForm,
    handleCreate,
    handleAuthorizeOAuth,
    openCreateAccount,
    openReauthorizeAccount,
    clearCreate,
  }
}
