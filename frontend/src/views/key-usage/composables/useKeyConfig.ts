// 本项目修改说明与原始来源见仓库根目录 NOTICE
import { onScopeDispose, shallowRef, watch } from 'vue'
import { getKeyUsageConfig } from '@/api/modules/key-usage'
import { useRequestState } from '@/composables/useRequestState'

export function useKeyConfig() {
  const showConfig = shallowRef(false)
  const configurationKey = shallowRef('')
  const request = useRequestState()
  async function openConfig() {
    const id = request.start()
    try {
      const result = await getKeyUsageConfig({ signal: request.signal })
      if (!request.isCurrent(id))
        return
      configurationKey.value = result.plaintextKey
      showConfig.value = true
    }
    catch (error) {
      request.fail(id, error)
    }
    finally {
      request.finish(id)
    }
  }
  watch(showConfig, (open) => {
    if (!open)
      configurationKey.value = ''
  })
  onScopeDispose(() => {
    configurationKey.value = ''
  })
  return {
    showConfig,
    configurationKey,
    openConfig,
    configuring: request.loading,
    configError: request.error,
  }
}
