// 本项目修改说明与原始来源见仓库根目录 NOTICE
import type { Account, AccountModelAccess } from '@/api'
import { ref, shallowRef, watch } from 'vue'

import { updateAccount } from '@/api'
import { useAsyncAction } from '@/composables/useAsyncAction'
import { t } from '@/i18n'
import { notification as ZNotification } from '@/utils/notification'
import { accountModelAccessError } from '../utils/modelAccess'
import { concurrencyLimitInput, parseAccountSchedulingForm } from '../utils/schedulingForm'

export function useAccountEditor(options: {
  reloadAccounts: () => Promise<unknown>
  reloadGroups: () => Promise<unknown>
}) {
  const showEditModal = shallowRef(false)
  // 保存后列表可能因筛选移除该账号，编辑窗口的退场仍需保留原账号内容。
  const editingAccount = shallowRef<Account | null>(null)
  const notes = shallowRef('')
  const schedulingEnabled = shallowRef(true)
  const concurrencyLimit = shallowRef('')
  const weight = shallowRef('1')
  const modelAccess = ref<AccountModelAccess | undefined>()
  const proxyMode = shallowRef('preserve')
  const proxyId = shallowRef('')
  const selectedGroupIds = ref<string[]>([])
  const saveAction = useAsyncAction()
  const saving = saveAction.loading
  const setupToken = ref({ token: '', expiresAt: '' })

  function open(account: Account) {
    clearCredentials()
    editingAccount.value = account
    notes.value = account.notes ?? ''
    proxyMode.value = 'preserve'
    proxyId.value = ''
    schedulingEnabled.value = account.enabled
    concurrencyLimit.value = concurrencyLimitInput(account.concurrencyLimit)
    weight.value = String(account.weight)
    modelAccess.value = {
      ...account.modelAccess,
      models: [...account.modelAccess.models],
    }
    selectedGroupIds.value = account.groups.map(group => group.id)
    showEditModal.value = true
  }

  async function save() {
    const accountId = editingAccount.value?.id
    if (!accountId || saving.value)
      return
    if (proxyMode.value === 'direct' || (proxyMode.value === 'proxy' && !proxyId.value.trim())) {
      ZNotification.warning({
        message: t('accounts.claude.proxyRequired'),
      })
      return
    }
    const modelError = accountModelAccessError(modelAccess.value)
    if (modelError) {
      ZNotification.warning({ message: modelError })
      return
    }
    const scheduling = parseAccountSchedulingForm(concurrencyLimit.value, weight.value)
    if (proxyMode.value === 'proxy' && !proxyId.value.trim()) {
      ZNotification.warning({
        message: t('accounts.batchEdit.proxyRequired'),
      })
      return
    }
    if (!scheduling.valid) {
      ZNotification.warning({ message: scheduling.message })
      return
    }

    await saveAction.run(async () => {
      const settings = {
        accountId,
        notes: notes.value,
        outboundProxyId: proxyMode.value === 'preserve'
          ? undefined
          : proxyMode.value === 'direct'
            ? ''
            : proxyId.value.trim(),
        enabled: schedulingEnabled.value,
        concurrencyLimit: scheduling.values.concurrencyLimit,
        weight: scheduling.values.weight,
        modelAccess: modelAccess.value,
        groupIds: [
          ...new Set(selectedGroupIds.value),
        ],
      }
      await updateAccount({
        ...settings,
        credential: editingAccount.value?.authenticationKind === 'setup_token' && setupToken.value.token.trim()
          ? {
              access_token: setupToken.value.token.trim(),
              expires_at: setupToken.value.expiresAt
                ? new Date(setupToken.value.expiresAt).toISOString()
                : null,
            }
          : undefined,
      })
      showEditModal.value = false
      ZNotification.success({
        message: t('accounts.editor.saved'),
      })
      void Promise.allSettled([
        options.reloadAccounts(),
        options.reloadGroups(),
      ])
    })
  }

  watch(showEditModal, (open) => {
    if (!open)
      clearCredentials()
  })

  function clearCredentials() {
    setupToken.value = { token: '', expiresAt: '' }
  }

  return {
    setupToken,
    showEditModal,
    editingAccount,
    notes,
    schedulingEnabled,
    concurrencyLimit,
    weight,
    modelAccess,
    proxyMode,
    proxyId,
    selectedGroupIds,
    saving,
    open,
    save,
    clearCredentials,
  }
}
