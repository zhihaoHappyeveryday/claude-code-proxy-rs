<script setup lang="ts">
import type { ClaudeClientIdentity, ClaudeClientProfile, ClaudeIdentityOptions, ClaudeIdentityPreview } from '@/api/modules/settings/claude-identity'
import type { ProviderRequestProfile } from '@/api/modules/settings/profiles'
import { ZButton, ZCard, ZForm, ZFormItem, ZInput, ZSelect } from '@codex-proxy/ui'
import { computed, onMounted, shallowRef, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { getClaudeIdentityOptions, previewClaudeIdentity } from '@/api/modules/settings/claude-identity'

const props = defineProps<{ disabled: boolean, account?: boolean }>()
const model = defineModel<ProviderRequestProfile>()
const { t } = useI18n({ useScope: 'global' })
const options = shallowRef<ClaudeIdentityOptions>()
const preview = shallowRef<ClaudeIdentityPreview>()
const loadError = shallowRef(false)
const previewError = shallowRef(false)
const previewing = shallowRef(false)
const configuration = computed(() => options.value
  ? (model.value ?? options.value.defaults) as ClaudeClientProfile
  : undefined)
const fieldsDisabled = computed(() => props.disabled || !options.value)
const identityDisabled = computed(() => fieldsDisabled.value || configuration.value?.tls.mode === 'emulate')
const tlsOptions = computed(() => [
  {
    label: t('settings.claudeIdentity.tlsStandard'),
    value: 'standard',
  },
  ...(options.value?.tlsTemplates ?? []).map(template => ({ label: `${t('settings.claudeIdentity.tlsEmulate')} · ${template.label}`, value: template.id })),
])
const tlsSelection = computed(() => configuration.value?.tls.mode === 'emulate'
  ? configuration.value.tls.template.id
  : 'standard')
const selectedTemplate = computed(() => options.value?.tlsTemplates.find(template => template.id === tlsSelection.value))
const osOptions = ['Linux', 'MacOS', 'Windows'].map(value => ({ label: value, value }))
const archOptions = ['arm64', 'x64'].map(value => ({ label: value, value }))
const versionFields = computed(() => [
  {
    field: 'cliVersion' as const,
    label: t('settings.claudeIdentity.cliVersion'),
  },
  {
    field: 'sdkVersion' as const,
    label: t('settings.claudeIdentity.sdkVersion'),
  },
  {
    field: 'nodeVersion' as const,
    label: t('settings.claudeIdentity.nodeVersion'),
  },
])

function update(field: keyof ClaudeClientIdentity, value: string | undefined) {
  if (configuration.value) {
    model.value = {
      ...configuration.value,
      httpIdentity: { ...configuration.value.httpIdentity, [field]: value ?? '' },
    }
  }
}

function updateTls(value: string | undefined) {
  if (!configuration.value || !value || value === tlsSelection.value)
    return
  const template = options.value?.tlsTemplates.find(item => item.id === value)
  model.value = template
    ? {
        schemaVersion: 1,
        httpIdentity: { ...template.identity },
        tls: {
          mode: 'emulate',
          template: { id: template.id, revision: template.revision },
        },
      }
    : {
        ...configuration.value,
        tls: { mode: 'standard' },
      }
}

async function load() {
  loadError.value = false
  try {
    options.value = await getClaudeIdentityOptions()
  }
  catch {
    loadError.value = true
  }
}

watch(configuration, (value, _, onCleanup) => {
  preview.value = undefined
  previewError.value = false
  previewing.value = !!value
  if (!value)
    return
  let stale = false
  const timer = window.setTimeout(async () => {
    try {
      const result = await previewClaudeIdentity(value)
      if (!stale)
        preview.value = result
    }
    catch {
      if (!stale)
        previewError.value = true
    }
    finally {
      if (!stale)
        previewing.value = false
    }
  }, 250)
  onCleanup(() => {
    stale = true
    window.clearTimeout(timer)
  })
})

onMounted(load)
</script>

<template>
  <ZCard id="claude-identity" :title="t('settings.claudeIdentity.title')">
    <div class="grid gap-5">
      <p class="m-0 max-w-4xl text-cp-sm leading-relaxed text-cp-text-secondary">
        {{ t('settings.claudeIdentity.description') }}
      </p>
      <div v-if="loadError" role="alert" class="flex items-center gap-3 text-cp-error">
        {{ t('settings.claudeIdentity.loadError') }}
        <ZButton @click="load">
          {{ t('settings.claudeIdentity.retry') }}
        </ZButton>
      </div>
      <ZFormItem :label="t('settings.claudeIdentity.tlsMode')">
        <ZSelect :model-value="tlsSelection" :aria-label="t('settings.claudeIdentity.tlsMode')" :options="tlsOptions" :disabled="fieldsDisabled" @update:model-value="updateTls" />
      </ZFormItem>
      <p class="m-0 text-cp-sm text-cp-text-secondary">
        {{ selectedTemplate ? t('settings.claudeIdentity.tlsVerified') : t('settings.claudeIdentity.tlsStandardHint') }}
      </p>
      <p v-if="selectedTemplate" class="m-0 text-cp-xs text-cp-text-secondary">
        {{ t('settings.claudeIdentity.tlsScope', { scope: `${selectedTemplate.label}；${selectedTemplate.evidenceScope}` }) }}
      </p>
      <ZForm class="max-w-6xl sm:grid-cols-3">
        <ZFormItem v-for="{ field, label } in versionFields" :key="field" :label="label">
          <ZInput
            :model-value="configuration?.httpIdentity[field] ?? ''"
            :aria-label="label"
            :disabled="identityDisabled"
            placeholder="X.Y.Z"
            @update:model-value="update(field, $event)"
          />
        </ZFormItem>
        <ZFormItem :label="t('settings.claudeIdentity.os')">
          <ZSelect :model-value="configuration?.httpIdentity.os" :aria-label="t('settings.claudeIdentity.os')" :options="osOptions" :disabled="identityDisabled" @update:model-value="update('os', $event)" />
        </ZFormItem>
        <ZFormItem :label="t('settings.claudeIdentity.arch')">
          <ZSelect :model-value="configuration?.httpIdentity.arch" :aria-label="t('settings.claudeIdentity.arch')" :options="archOptions" :disabled="identityDisabled" @update:model-value="update('arch', $event)" />
        </ZFormItem>
      </ZForm>
      <div aria-live="polite" class="grid gap-3 rounded-cp-lg bg-cp-fill-alter/70 p-4">
        <div class="flex flex-wrap items-center justify-between gap-2">
          <span class="text-cp-sm font-bold">{{ t('settings.claudeIdentity.preview') }}</span>
          <span v-if="preview" class="text-cp-xs text-cp-text-secondary">{{ preview.versionSource === 'local-capture' ? t('settings.claudeIdentity.reference') : t('settings.claudeIdentity.custom') }}</span>
        </div>
        <span v-if="previewing" class="text-cp-sm text-cp-text-secondary">{{ t('settings.claudeIdentity.previewing') }}</span>
        <span v-else-if="previewError" role="alert" class="text-cp-sm text-cp-error">{{ t('settings.claudeIdentity.invalid') }}</span>
        <template v-else-if="preview">
          <code class="text-cp-sm break-all text-cp-text">{{ preview.userAgent }}</code>
          <dl class="m-0 grid gap-2 text-cp-xs sm:grid-cols-2">
            <div v-for="(value, header) in preview.headers" :key="header" class="flex min-w-0 flex-wrap gap-x-2">
              <dt class="text-cp-text-secondary">
                {{ header }}
              </dt>
              <dd class="m-0 font-mono break-all text-cp-text">
                {{ value }}
              </dd>
            </div>
          </dl>
        </template>
      </div>
      <div class="flex flex-wrap items-center justify-between gap-3">
        <p class="m-0 text-cp-xs text-cp-text-secondary">
          {{ account ? t('settings.claudeIdentity.accountSaveHint') : t('settings.claudeIdentity.saveHint') }}
        </p>
        <ZButton :disabled="fieldsDisabled || !model" @click="model = undefined">
          {{ t('settings.claudeIdentity.restore') }}
        </ZButton>
      </div>
    </div>
  </ZCard>
</template>
