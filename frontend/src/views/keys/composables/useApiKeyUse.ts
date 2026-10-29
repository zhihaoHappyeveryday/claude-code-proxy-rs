// 本项目修改说明与原始来源见仓库根目录 NOTICE
import type { ApiKey } from '@/api'
import { onScopeDispose, shallowRef, watch } from 'vue'
import { revealApiKey } from '@/api'
import { useCopyText } from '@/composables/useCopyText'
import { useRequestState } from '@/composables/useRequestState'
import { t } from '@/i18n'
import { notification as ZNotification } from '@/utils/notification'

type KeyRevealAction = 'copy' | 'configure'

// 明文读取、复制、展示与导入共用生命周期，列表数据不保存明文。
export function useApiKeyUse() {
  const copyText = useCopyText()
  const revealingKeys = shallowRef(new Map<string, KeyRevealAction>())
  const revealRequest = useRequestState()
  const showUseKeyModal = shallowRef(false)
  const showCreatedKeyModal = shallowRef(false)
  const createdKey = shallowRef('')
  const configurationKey = shallowRef('')

  function showCreatedKey(key: string) {
    createdKey.value = key
    showCreatedKeyModal.value = true
  }

  function openUseKeyModal(apiKey: ApiKey) {
    return withPlaintextKey(apiKey, 'configure', (key) => {
      configurationKey.value = key
      showUseKeyModal.value = true
    })
  }

  watch(showUseKeyModal, (open) => {
    if (!open)
      configurationKey.value = ''
  })

  async function copyToClipboard(text: string) {
    await copyText(text, {
      successText: t('keys.configuration.copiedToClipboard'),
      emptyErrorText: t('common.errors.copy'),
    })
  }

  async function withPlaintextKey(apiKey: ApiKey, action: KeyRevealAction, use: (key: string) => void | Promise<void>) {
    if (revealingKeys.value.has(apiKey.id))
      return
    // 明文操作共用一个当前意图，跨行点击和页面离开都使旧结果失效
    const requestId = revealRequest.start()
    revealingKeys.value = new Map(revealingKeys.value).set(apiKey.id, action)
    try {
      const result = await revealApiKey({ id: apiKey.id }, { signal: revealRequest.signal })
      if (!revealRequest.isCurrent(requestId))
        return
      if (!result.plaintextKey) {
        ZNotification.error({
          message: t('keys.configuration.fullApiKeyNotAvailable'),
        })
        return
      }
      await use(result.plaintextKey)
    }
    catch (error) {
      revealRequest.fail(requestId, error)
    }
    finally {
      const pendingKeys = new Map(revealingKeys.value)
      pendingKeys.delete(apiKey.id)
      revealingKeys.value = pendingKeys
      revealRequest.finish(requestId)
    }
  }

  function copyApiKey(apiKey: ApiKey) {
    return withPlaintextKey(apiKey, 'copy', copyToClipboard)
  }

  function clearCreatedKey() {
    createdKey.value = ''
  }

  onScopeDispose(() => {
    clearCreatedKey()
    configurationKey.value = ''
  })

  return {
    showUseKeyModal,
    configurationKey,
    showCreatedKeyModal,
    createdKey,
    revealingKeys,
    showCreatedKey,
    clearCreatedKey,
    copyToClipboard,
    copyApiKey,
    openUseKeyModal,
  }
}
