<!-- 本项目修改说明与原始来源见仓库根目录 NOTICE -->
<script setup lang="ts">
import { ZButton, ZDialog, ZInput } from '@codex-proxy/ui'
import { computed, shallowRef, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { useCopyText } from '@/composables/useCopyText'

const props = defineProps<{
  title?: string
  apiKey?: string
}>()
const emit = defineEmits<{ afterLeave: [] }>()
const open = defineModel<boolean>({ default: false })
const { t } = useI18n({ useScope: 'global' })
const baseUrl = shallowRef(window.location.origin)
const copyText = useCopyText()
const validBase = computed(() => {
  try {
    const url = new URL(baseUrl.value)
    return ['http:', 'https:'].includes(url.protocol) && !url.username && !url.password && !url.search && !url.hash
  }
  catch { return false }
})
function quote(value: string) {
  return `'${value.replace(/'/g, '\'\\\'\'')}'`
}
const configuration = computed(() => open.value && props.apiKey && validBase.value
  ? `export ANTHROPIC_BASE_URL=${quote(baseUrl.value.replace(/\/+$/, ''))}\nexport ANTHROPIC_API_KEY=${quote(props.apiKey)}\nunset ANTHROPIC_AUTH_TOKEN`
  : '')
watch(open, (value) => {
  if (value)
    baseUrl.value = window.location.origin
})
function copyConfiguration() {
  return copyText(configuration.value, {
    successText: t('keys.configuration.copiedToClipboard'),
    emptyErrorText: t('common.errors.copy'),
  })
}
</script>

<template>
  <ZDialog
    v-model="open"
    :title="title ?? 'Claude Code'"
    :description="t('common.claudeConfiguration.description')"
    width="42rem"
    @closed="emit('afterLeave')"
  >
    <div class="flex min-w-0 flex-col gap-3">
      <span>{{ t('common.claudeConfiguration.baseUrl') }}</span>
      <ZInput id="claude-base-url" v-model="baseUrl" :aria-label="t('common.claudeConfiguration.baseUrl')" :maxlength="2048" />
      <p v-if="!validBase" role="alert" class="m-0 text-cp-error-text">
        {{ t('common.claudeConfiguration.invalidUrl') }}
      </p>
      <pre class="m-0 overflow-x-auto rounded-cp-lg bg-cp-fill px-4 py-3 text-cp-sm"><code>{{ configuration }}</code></pre>
      <p class="m-0 text-cp-sm text-cp-text-tertiary">
        {{ t('common.claudeConfiguration.keyHint') }}
      </p>
    </div>
    <template #footer>
      <ZButton @click="open = false">
        {{ t('common.actions.close') }}
      </ZButton>
      <ZButton type="primary" :disabled="!configuration" @click="copyConfiguration">
        {{ t('common.claudeConfiguration.copy') }}
      </ZButton>
    </template>
  </ZDialog>
</template>
