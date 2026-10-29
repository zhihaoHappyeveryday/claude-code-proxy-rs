<!-- 本项目修改说明与原始来源见仓库根目录 NOTICE -->
<script setup lang="ts">
import type { AccountCreateSource } from '../../utils/accountCreate'

import { ZSegmented } from '@codex-proxy/ui'
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import { formatProviderLabel, PROVIDER_IDS, providerIcon } from '@/utils/providers'

defineProps<{ disabled: boolean }>()

const { t } = useI18n({ useScope: 'global' })

const source = defineModel<AccountCreateSource | null>({ required: true })
const options = computed(() => PROVIDER_IDS.map(provider => ({
  value: `provider:${provider}`,
  label: formatProviderLabel(provider),
  icon: providerIcon(provider),
})))
const selected = computed({
  get: () => source.value?.kind === 'provider'
    ? `provider:${source.value.id}`
    : source.value?.kind ?? '',
  set: (value: string) => {
    if (!options.value.some(option => option.value === value))
      return
    source.value = {
      kind: 'provider',
      id: value.slice('provider:'.length),
    }
  },
})
</script>

<template>
  <ZSegmented
    v-model="selected"
    class="w-full"
    :aria-label="t('accounts.onboarding.selectAccountPlatform')"
    :options="options"
    :disabled="disabled"
    size="large"
  />
</template>
