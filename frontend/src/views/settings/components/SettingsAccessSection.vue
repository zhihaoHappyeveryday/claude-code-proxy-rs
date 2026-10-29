<!-- 本项目修改说明与原始来源见仓库根目录 NOTICE -->
<script setup lang="ts">
import { ZConfirmDialog } from '@codex-proxy/ui'

import { onMounted } from 'vue'
import { useI18n } from 'vue-i18n'
import { useAdminApiKey } from '../composables/useAdminApiKey'
import AdminApiKeyCard from './AdminApiKeyCard.vue'
import AdminPasswordCard from './AdminPasswordCard.vue'
import ResponseBodyLimitCard from './ResponseBodyLimitCard.vue'

defineProps<{
  disabled: boolean
  loading: boolean
  desktopError: string
  cliError: string
}>()

const { t } = useI18n({ useScope: 'global' })

const responsesMaxDecompressedBodyMiB = defineModel<string>('responsesMaxDecompressedBodyMiB', { required: true })

const {
  loading: adminKeyLoading,
  regenerating,
  deleting,
  showDeleteModal,
  generatedKey,
  status,
  regenerate,
  remove,
  copyGeneratedKey,
  loadStatus,
} = useAdminApiKey()

onMounted(loadStatus)
</script>

<template>
  <div class="grid min-w-0 gap-5">
    <AdminPasswordCard />

    <AdminApiKeyCard
      :status="status"
      :loading="adminKeyLoading"
      :regenerating="regenerating"
      :deleting="deleting"
      :generated-key="generatedKey"
      @regenerate="regenerate"
      @request-delete="showDeleteModal = true"
      @copy="copyGeneratedKey"
    />

    <fieldset :disabled="disabled" class="m-0 grid min-w-0 gap-5 border-0 p-0" :aria-label="t('settings.access.accessRestrictions')">
      <ResponseBodyLimitCard v-model="responsesMaxDecompressedBodyMiB" />
    </fieldset>

    <ZConfirmDialog
      v-model="showDeleteModal"
      :title="t('settings.access.deleteAdminApiKey')"
      :description="t('settings.access.deleteKeyDescription')"
      destructive
      :confirm-text="t('common.actions.delete')"
      :loading="deleting"
      @confirm="remove"
    >
      <p class="m-0">
        {{ t('settings.access.areYouSureYouWantToDelete') }}
      </p>
    </ZConfirmDialog>
  </div>
</template>
