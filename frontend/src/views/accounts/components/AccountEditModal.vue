<!-- 本项目修改说明与原始来源见仓库根目录 NOTICE -->
<script setup lang="ts">
import type { Account, AccountGroupRef, AccountModelAccess } from '@/api'

import { ZButton, ZDialog, ZFormItem, ZInput, ZTextarea } from '@codex-proxy/ui'

import { useI18n } from 'vue-i18n'
import AccountPlanBadge from '@/components/account/AccountPlanBadge.vue'
import ProviderIconGroup from '@/components/ProviderIconGroup.vue'
import AccountIdentityCell from './AccountIdentityCell.vue'
import AccountSettingsFields from './AccountSettingsFields.vue'

defineProps<{
  account: Account | null
  groups: AccountGroupRef[]
  groupsLoading: boolean
  saving: boolean
}>()

const emit = defineEmits<{
  save: []
}>()

const { t } = useI18n({ useScope: 'global' })

const open = defineModel<boolean>({ required: true })
const setupToken = defineModel<{ token: string, expiresAt: string }>('setupToken', { required: true })
const notes = defineModel<string>('notes', { required: true })
const enabled = defineModel<boolean>('enabled', { required: true })
const concurrencyLimit = defineModel<string>('concurrencyLimit', { required: true })
const modelAccess = defineModel<AccountModelAccess | undefined>('modelAccess', { required: true })
const weight = defineModel<string>('weight', { required: true })
const proxyMode = defineModel<string>('proxyMode', { required: true })
const proxyId = defineModel<string>('proxyId', { required: true })
const selectedGroupIds = defineModel<string[]>('selectedGroupIds', { required: true })
</script>

<template>
  <ZDialog
    v-model="open"
    :title="t('accounts.editor.title')"
    width="42rem"
    :show-close="!saving" :close-on-click-modal="!saving" :close-on-press-escape="!saving"
  >
    <div v-if="account" class="grid gap-5">
      <div
        class="flex flex-wrap items-center justify-between gap-4 rounded-cp bg-cp-fill-quaternary px-4 py-3.5"
      >
        <AccountIdentityCell
          class="min-w-0 flex-1"
          :account="account"
          size="lg"
        />
        <div class="flex shrink-0 items-center gap-3">
          <AccountPlanBadge :authentication-kind="account.authenticationKind" :plan-type="account.planType" :plan-type-display="account.planTypeDisplay" size="sm" />
          <ProviderIconGroup
            :provider="account.provider"
            :authentication-kind="account.authenticationKind"
          />
        </div>
      </div>

      <section v-if="account.authenticationKind === 'setup_token'" class="grid gap-4">
        <ZFormItem :label="t('accounts.claude.replaceToken')" :description="t('accounts.claude.replaceHelp')">
          <ZInput v-model="setupToken.token" type="password" autocomplete="off" :aria-label="t('accounts.claude.replaceToken')" :disabled="saving" />
        </ZFormItem>
        <ZFormItem :label="t('accounts.claude.expiresAt')" :description="t('accounts.claude.expiryHelp')">
          <ZInput v-model="setupToken.expiresAt" type="datetime-local" :aria-label="t('accounts.claude.expiresAt')" :disabled="saving || !setupToken.token" />
        </ZFormItem>
      </section>

      <AccountSettingsFields
        v-model:enabled="enabled"
        v-model:concurrency-limit="concurrencyLimit"
        v-model:weight="weight"
        v-model:model-access="modelAccess"
        v-model:selected-group-ids="selectedGroupIds"
        v-model:proxy-mode="proxyMode"
        v-model:proxy-id="proxyId"
        :groups="groups"
        :groups-loading="groupsLoading"
        :disabled="saving"
        :show-scheduling="false"
        :current-proxy-id="account.outboundProxyId"
        :endpoint="account.outboundProxyEndpoint"
        :account-id="account.id"
      />

      <ZFormItem :label="t('common.fields.notes')">
        <ZTextarea
          v-model="notes"
          :rows="3"
          :maxlength="500"
          :placeholder="t('accounts.editor.notesHelp')"
          :disabled="saving"
        />
      </ZFormItem>
    </div>

    <template #footer>
      <ZButton :disabled="saving" @click="open = false">
        {{ t('common.actions.cancel') }}
      </ZButton>
      <ZButton
        type="primary"
        :loading="saving"
        :disabled="!account || groupsLoading"
        @click="emit('save')"
      >
        {{ t('common.actions.save') }}
      </ZButton>
    </template>
  </ZDialog>
</template>
