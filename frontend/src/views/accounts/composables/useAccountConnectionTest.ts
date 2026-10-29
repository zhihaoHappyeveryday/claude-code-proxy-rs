// 本项目修改说明与原始来源见仓库根目录 NOTICE
import type { Account } from '@/api'
import { CheckCircle2, Clock3, Wifi, XCircle } from '@lucide/vue'
import { computed, ref, shallowRef, watch } from 'vue'
import { refreshAccountModels } from '@/api'
import { useRequestState } from '@/composables/useRequestState'
import { t } from '@/i18n'
import { errorMessage } from '@/utils/operation'

type Status = 'idle' | 'running' | 'success' | 'error'
interface ConnectionLog { key: string, time: string, text: string, tone: string, detail: string }

export function useAccountConnectionTest(options: { reload: () => Promise<unknown> }) {
  const showConnectionTestModal = shallowRef(false)
  const testingAccount = shallowRef<Account | null>(null)
  const connectionTestStatus = shallowRef<Status>('idle')
  const connectionTestModel = shallowRef('')
  const connectionTestLogs = ref<ConnectionLog[]>([])
  const connectionTestError = shallowRef('')
  const connectionTestStartedAt = shallowRef('')
  const connectionTestFinishedAt = shallowRef('')
  const connectionTestDurationMs = shallowRef<number | null>(null)
  const request = useRequestState()
  const testingConnectionIds = computed(() => new Set(
    connectionTestStatus.value === 'running' && testingAccount.value
      ? [testingAccount.value.id]
      : [],
  ))
  const connectionTestStatusView = computed(() => {
    const state = {
      idle: {
        label: t('accounts.connectionTest.readyTitle'),
        description: t('accounts.connectionTest.readyDescription'),
        icon: Wifi,
        badge: 'bg-cp-fill-tertiary text-cp-text-secondary',
      },
      running: {
        label: t('accounts.connectionTest.testing'),
        description: t('accounts.connectionTest.runningDescription'),
        icon: Clock3,
        badge: 'bg-cp-info-container text-cp-info-on-container',
      },
      success: {
        label: t('accounts.connectionTest.successTitle'),
        description: t('accounts.connectionTest.completedDescription'),
        icon: CheckCircle2,
        badge: 'bg-cp-success-container text-cp-success-on-container',
      },
      error: {
        label: t('accounts.connectionTest.failed'),
        description: t('accounts.connectionTest.failedDescription'),
        icon: XCircle,
        badge: 'bg-cp-error-container text-cp-error-on-container',
      },
    }[connectionTestStatus.value]
    return { ...state, iconClass: '' }
  })

  function log(text: string, tone: string, detail = '') {
    connectionTestLogs.value.push({
      key: String(connectionTestLogs.value.length),
      time: new Date().toLocaleTimeString(),
      text,
      tone,
      detail,
    })
  }

  function openConnectionTest(account: Account) {
    request.invalidate()
    testingAccount.value = account
    connectionTestStatus.value = 'idle'
    connectionTestModel.value = ''
    connectionTestLogs.value = []
    connectionTestError.value = ''
    connectionTestStartedAt.value = ''
    connectionTestFinishedAt.value = ''
    connectionTestDurationMs.value = null
    showConnectionTestModal.value = true
  }

  async function handleTestConnection() {
    const account = testingAccount.value
    if (!account || request.loading.value)
      return
    const id = request.start()
    const started = performance.now()
    connectionTestStatus.value = 'running'
    connectionTestStartedAt.value = new Date().toLocaleString()
    connectionTestFinishedAt.value = ''
    connectionTestDurationMs.value = null
    connectionTestError.value = ''
    connectionTestModel.value = ''
    connectionTestLogs.value = []
    log(t('accounts.connectionTest.events.request'), 'info', 'GET /v1/models')
    try {
      const result = await refreshAccountModels({ accountId: account.id }, { signal: request.signal, silent: true })
      if (!request.isCurrent(id))
        return
      connectionTestModel.value = result.models.map(model => model.id).join(', ')
      connectionTestStatus.value = 'success'
      log(t('accounts.connectionTest.modelsRefreshed', { count: result.models.length }), 'success', connectionTestModel.value)
    }
    catch (error) {
      if (!request.isCurrent(id))
        return
      connectionTestError.value = errorMessage(error, t('accounts.connectionTest.errors.failed'))
      connectionTestStatus.value = 'error'
      log(connectionTestError.value, 'danger')
    }
    finally {
      if (request.isCurrent(id)) {
        connectionTestFinishedAt.value = new Date().toLocaleString()
        connectionTestDurationMs.value = Math.round(performance.now() - started)
        request.finish(id)
        void options.reload()
      }
    }
  }

  watch(showConnectionTestModal, (open) => {
    if (!open) {
      request.invalidate()
      connectionTestStatus.value = 'idle'
    }
  })

  return {
    showConnectionTestModal,
    testingAccount,
    connectionTestStatus,
    connectionTestModel,
    connectionTestLogs,
    connectionTestError,
    connectionTestStartedAt,
    connectionTestFinishedAt,
    connectionTestDurationMs,
    testingConnectionIds,
    connectionTestStatusView,
    openConnectionTest,
    handleTestConnection,
  }
}
