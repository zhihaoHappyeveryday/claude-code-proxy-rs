<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

const props = defineProps<{ value: unknown }>()
const { t } = useI18n({ useScope: 'global' })
const observation = computed(() => {
  if (!props.value || typeof props.value !== 'object')
    return null
  const value = props.value as Record<string, unknown>
  const configuration = value.configuration as { tls?: { mode?: string, template?: { id?: string, revision?: number } } } | undefined
  return {
    source: value.source === 'account'
      ? t('settings.claudeIdentity.override')
      : t('settings.claudeIdentity.inherit'),
    applied: value.applied === true,
    phase: typeof value.phase === 'string'
      ? value.phase
      : '—',
    template: configuration?.tls?.mode === 'emulate'
      ? `${configuration.tls.template?.id ?? '—'} · r${configuration.tls.template?.revision ?? '—'}`
      : t('settings.claudeIdentity.tlsStandard'),
  }
})
</script>

<template>
  <section v-if="observation" class="grid gap-3 rounded-cp-card bg-cp-fill-quaternary px-4 py-3.5">
    <h3 class="m-0 text-cp-sm font-heavy">
      {{ t('settings.claudeIdentity.title') }}
    </h3>
    <dl class="m-0 grid gap-2 text-cp-sm sm:grid-cols-2">
      <div>
        <dt class="text-cp-text-secondary">
          {{ t('settings.claudeIdentity.configurationSource') }}
        </dt>
        <dd class="m-0">
          {{ observation.source }}
        </dd>
      </div>
      <div>
        <dt class="text-cp-text-secondary">
          {{ t('settings.claudeIdentity.requestState') }}
        </dt>
        <dd class="m-0">
          {{ observation.applied ? t('settings.claudeIdentity.applied') : t('settings.claudeIdentity.notApplied') }} · {{ observation.phase }}
        </dd>
      </div>
      <div class="sm:col-span-2">
        <dt class="text-cp-text-secondary">
          {{ t('settings.claudeIdentity.tlsTemplate') }}
        </dt>
        <dd class="m-0 break-all">
          {{ observation.template }}
        </dd>
      </div>
    </dl>
    <p class="m-0 text-cp-xs text-cp-text-secondary">
      {{ t('settings.claudeIdentity.observationHint') }}
    </p>
  </section>
</template>
