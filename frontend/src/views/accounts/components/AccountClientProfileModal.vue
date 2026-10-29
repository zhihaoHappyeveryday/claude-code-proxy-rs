<script setup lang="ts">
import type { Account } from '@/api'
import type { ClaudeClientProfile, ClaudeIdentityOptions } from '@/api/modules/settings/claude-identity'
import type { ProviderRequestProfile } from '@/api/modules/settings/profiles'
import { ZButton, ZDialog, ZFormItem, ZSelect } from '@codex-proxy/ui'
import { computed, shallowRef, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { getAccountClientProfile, getClaudeIdentityOptions, saveAccountClientProfile } from '@/api/modules/settings/claude-identity'
import ClaudeIdentityCard from '@/views/settings/components/ClaudeIdentityCard.vue'

const props = defineProps<{ account: Account | null }>()
const open = defineModel<boolean>({ required: true })
const { t } = useI18n({ useScope: 'global' })
const loading = shallowRef(false)
const saving = shallowRef(false)
const error = shallowRef('')
const revision = shallowRef<number>()
const options = shallowRef<ClaudeIdentityOptions>()
const source = shallowRef('platform')
const configuration = shallowRef<ProviderRequestProfile>()
const sources = computed(() => [
  {
    value: 'platform',
    label: t('settings.claudeIdentity.inherit'),
  },
  {
    value: 'account',
    label: t('settings.claudeIdentity.override'),
  },
])

watch([
  open,
  () => props.account?.id,
], async ([visible, id], _, onCleanup) => {
  if (!visible || !id)
    return
  let stale = false
  onCleanup(() => {
    stale = true
  })
  loading.value = true
  error.value = ''
  revision.value = undefined
  try {
    const [current, available] = await Promise.all([
      getAccountClientProfile(id),
      getClaudeIdentityOptions(),
    ])
    if (stale)
      return
    options.value = available
    revision.value = current.revision
    source.value = current.configuration ? 'account' : 'platform'
    configuration.value = structuredClone(current.configuration ?? available.globalConfiguration ?? available.defaults)
  }
  catch {
    if (!stale)
      error.value = t('settings.claudeIdentity.loadError')
  }
  finally {
    if (!stale)
      loading.value = false
  }
})

async function save() {
  if (!props.account || !revision.value || !options.value)
    return
  saving.value = true
  error.value = ''
  try {
    const value = source.value === 'account'
      ? (configuration.value ?? options.value.defaults) as ClaudeClientProfile
      : null
    await saveAccountClientProfile(props.account.id, revision.value, value)
    open.value = false
  }
  catch (cause) {
    error.value = cause instanceof Error
      ? cause.message
      : t('settings.claudeIdentity.saveError')
  }
  finally {
    saving.value = false
  }
}
</script>

<template>
  <ZDialog v-model="open" :title="t('settings.claudeIdentity.accountTitle')" width="56rem" :show-close="!saving" :close-on-click-modal="!saving" :close-on-press-escape="!saving">
    <div class="grid gap-4">
      <p v-if="account" class="m-0 font-semibold">
        {{ account.name }}
      </p>
      <p v-if="loading" role="status">
        {{ t('settings.claudeIdentity.previewing') }}
      </p>
      <p v-if="error" role="alert" class="m-0 text-cp-error">
        {{ error }}
      </p>
      <template v-if="revision">
        <ZFormItem :label="t('settings.claudeIdentity.configurationSource')">
          <ZSelect :model-value="source" :aria-label="t('settings.claudeIdentity.configurationSource')" :options="sources" :disabled="saving" @update:model-value="source = $event ?? 'platform'" />
        </ZFormItem>
        <p class="m-0 text-cp-sm text-cp-text-secondary">
          {{ t('settings.claudeIdentity.inheritHint') }}
        </p>
        <ClaudeIdentityCard v-if="source === 'account'" v-model="configuration" :disabled="saving" account />
      </template>
    </div>
    <template #footer>
      <ZButton :disabled="saving" @click="open = false">
        {{ t('common.actions.cancel') }}
      </ZButton>
      <ZButton type="primary" :loading="saving" :disabled="loading || !revision" @click="save">
        {{ t('common.actions.save') }}
      </ZButton>
    </template>
  </ZDialog>
</template>
