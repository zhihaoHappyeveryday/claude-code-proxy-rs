<!-- 本项目修改说明与原始来源见仓库根目录 NOTICE -->
<script setup lang="ts">
import type { AccountAuthorizationView } from '../../composables/useAccountAuthorization'

import type { AccountCreateForm, AccountImportMode } from '../../utils/accountCreate'
import type { Account, AccountGroupRef } from '@/api'

import { ZButton, ZDialog, ZFormItem, ZIconButton, ZInput, ZSegmented } from '@codex-proxy/ui'
import { Copy, LayoutGrid, Settings2, Sparkles } from '@lucide/vue'
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import AccountPlanBadge from '@/components/account/AccountPlanBadge.vue'
import ProviderIconGroup from '@/components/ProviderIconGroup.vue'
import { useCopyText } from '@/composables/useCopyText'
import { accountCreateProvider, accountProxyError, usesAuthorization } from '../../utils/accountCreate'
import { accountModelAccessError } from '../../utils/modelAccess'
import { parseAccountSchedulingForm } from '../../utils/schedulingForm'
import AccountIdentityCell from '../AccountIdentityCell.vue'
import AccountOAuthFields from './AccountOAuthFields.vue'
import AccountSetupFields from './AccountSetupFields.vue'
import { resolveAccountCreatePresentation } from './presenter'

const props = withDefaults(defineProps<{
  groups: AccountGroupRef[]
  groupsLoading: boolean
  saving?: boolean
  oauthLoading?: boolean
  reauthorizing?: boolean
  account?: Account | null
  authorization: AccountAuthorizationView
}>(), {
  saving: false,
  oauthLoading: false,
  reauthorizing: false,
  account: null,
})

const emit = defineEmits<{ create: [], generateOauth: [] }>()

const { t } = useI18n({ useScope: 'global' })

const open = defineModel<boolean>({ default: false })
const form = defineModel<AccountCreateForm>('form', { required: true })
const callback = defineModel<string>('callback', { required: true })
const copyWithToast = useCopyText()
const accountCopyValue = computed(() =>
  props.account?.email?.trim()
  || props.account?.accountId?.trim()
  || props.account?.id
  || '',
)
const busy = computed(() => props.saving || props.oauthLoading)
const canSelect = computed(() => Boolean(accountCreateProvider(form.value)))
const proxyError = computed(() => accountProxyError(form.value))
const modelError = computed(() => accountModelAccessError(form.value.modelAccess))
const scheduling = computed(() => parseAccountSchedulingForm(form.value.concurrencyLimit, form.value.weight))
const view = computed(() => resolveAccountCreatePresentation({
  form: form.value,
  provider: accountCreateProvider(form.value),
  authorization: props.authorization,
  callback: callback.value,
  busy: busy.value,
  reauthorizing: props.reauthorizing,
}))
const mode = computed({
  get: () => form.value.mode,
  set: (value: string) => {
    if (!props.reauthorizing && view.value.modeOptions.some(option => option.value === value))
      form.value.mode = value as AccountImportMode
  },
})

function continueToImport() {
  if (form.value.name.trim() && canSelect.value && !modelError.value && scheduling.value.valid && !props.groupsLoading && !proxyError.value && !busy.value)
    form.value.step = 'import'
}
</script>

<template>
  <ZDialog
    v-model="open"
    :title="view.modal.title"
    :description="view.modal.description"
    :type="view.modal.type"
    :width="view.modal.width"
    :show-close="!busy" :close-on-click-modal="!busy" :close-on-press-escape="!busy"
  >
    <template #icon>
      <Settings2 v-if="view.configuring" class="text-cp-text" :size="20" aria-hidden="true" />
      <LayoutGrid v-else-if="view.isBatch" class="text-cp-text" :size="20" aria-hidden="true" />
      <Sparkles v-else class="text-cp-text" :size="20" aria-hidden="true" />
    </template>

    <div class="grid gap-4">
      <div
        v-if="reauthorizing && account"
        class="flex flex-wrap items-center justify-between gap-4 rounded-cp bg-cp-fill-quaternary px-4 py-3.5"
      >
        <AccountIdentityCell
          class="min-w-0 flex-1"
          :account="account"
          size="lg"
        />
        <div class="flex shrink-0 items-center gap-3">
          <AccountPlanBadge
            :plan-type="account.planType"
            :plan-type-display="account.planTypeDisplay"
            size="sm"
          />
          <ProviderIconGroup
            :provider="account.provider"
            :authentication-kind="account.authenticationKind"
          />
          <ZIconButton
            variant="solid"
            size="small"
            :aria-label="t('accounts.onboarding.copyAccount')"
            :disabled="busy || !accountCopyValue"
            @click="copyWithToast(accountCopyValue, { successText: t('accounts.onboarding.accountCopied') })"
          >
            <Copy class="size-3.5" />
          </ZIconButton>
        </div>
      </div>

      <AccountSetupFields
        v-if="view.configuring"
        v-model="form"
        :disabled="busy"
        :groups="groups"
        :groups-loading="groupsLoading"
        :proxy-error="proxyError"
      />
      <p v-if="view.configuring && !scheduling.valid" class="m-0 text-xs text-cp-error" role="alert">
        {{ scheduling.message }}
      </p>
      <template v-if="!view.configuring">
        <ZSegmented
          v-if="!reauthorizing && !view.isBatch"
          v-model="mode"
          :aria-label="t('accounts.onboarding.howToAddAccount')"
          :options="view.modeOptions"
          :disabled="busy"
          class="w-full"
        />
        <ZSegmented
          v-if="mode === 'setup_token' && !reauthorizing"
          v-model="form.setupToken.input"
          :aria-label="t('accounts.claude.tokenSource')"
          :options="[{ label: t('accounts.claude.authorizeToken'), value: 'authorize' }, { label: t('accounts.claude.pasteToken'), value: 'paste' }]"
          :disabled="busy"
        />
        <AccountOAuthFields
          v-if="usesAuthorization(form)"
          v-model="callback"
          :auth-url="authorization.flow?.authorizationUrl ?? ''"
          :authorization="authorization"
          :can-start="true"
          :panel-title="`${view.label} ${reauthorizing ? t('accounts.actions.reauthorize') : t('accounts.onboarding.authorize')}`"
          :panel-description="view.oauth.description"
          :loading="oauthLoading"
          :callback-label="view.oauth.callbackLabel"
          :callback-placeholder="view.oauth.callbackPlaceholder"
          :disabled="busy"
          @regenerate="emit('generateOauth')"
        />
        <template v-else-if="mode === 'setup_token'">
          <ZFormItem label="Setup Token" required :description="t('accounts.claude.tokenHelp')">
            <ZInput v-model="form.setupToken.token" type="password" autocomplete="off" aria-label="Setup Token" :disabled="busy" />
          </ZFormItem>
          <ZFormItem :label="t('accounts.claude.expiresAt')" :description="t('accounts.claude.expiryHelp')">
            <ZInput v-model="form.setupToken.expiresAt" type="datetime-local" :aria-label="t('accounts.claude.expiresAt')" :disabled="busy" />
          </ZFormItem>
        </template>
      </template>
    </div>

    <template #footer>
      <ZButton v-if="view.configuring || reauthorizing" :disabled="busy" @click="open = false">
        {{ t('common.actions.cancel') }}
      </ZButton>
      <ZButton v-else class="mr-auto" :disabled="busy" @click="form.step = 'settings'">
        {{ t('common.actions.previous') }}
      </ZButton>
      <ZButton v-if="view.configuring" type="primary" :disabled="!form.name.trim() || !canSelect || !scheduling.valid || Boolean(modelError) || groupsLoading || Boolean(proxyError) || busy" @click="continueToImport">
        {{ t('accounts.onboarding.continueImporting') }}
      </ZButton>
      <ZButton v-else type="primary" :loading="saving || oauthLoading" :disabled="!view.canSubmit" @click="emit('create')">
        {{ view.submitLabel }}
      </ZButton>
    </template>
  </ZDialog>
</template>
