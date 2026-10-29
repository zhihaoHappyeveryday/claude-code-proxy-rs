import request from '../../request'

export interface ClaudeClientIdentity extends Record<string, unknown> {
  cliVersion: string
  sdkVersion: string
  os: 'Linux' | 'MacOS' | 'Windows'
  arch: 'arm64' | 'x64'
  nodeVersion: string
}

export interface ClaudeIdentityOptions {
  defaults: ClaudeClientProfile
  globalConfiguration: ClaudeClientProfile
  tlsTemplates: ClaudeTlsTemplate[]
}

export type ClaudeTlsPolicy = { mode: 'standard' } | { mode: 'emulate', template: { id: string, revision: number } }

export interface ClaudeClientProfile extends Record<string, unknown> {
  schemaVersion: 1
  httpIdentity: ClaudeClientIdentity
  tls: ClaudeTlsPolicy
}

export interface ClaudeTlsTemplate {
  id: string
  revision: number
  label: string
  evidence: 'local-client-capture'
  evidenceScope: string
  runtimeDeclaration: string
  protocol: string
  identity: ClaudeClientIdentity
}

export interface ClaudeIdentityPreview {
  configuration: ClaudeClientProfile
  userAgent: string
  policy: 'native-first'
  versionSource: 'local-capture' | 'custom'
  tls: ClaudeTlsPolicy
  tlsEvidence: 'local-client-capture' | null
  headers: Record<string, string>
  verifiedAt: string | null
}

export function getClaudeIdentityOptions() {
  return request<ClaudeIdentityOptions>({
    url: '/api/admin/settings/client-profiles/claude',
    method: 'GET',
    silent: true,
  })
}

export function previewClaudeIdentity(configuration: ClaudeClientProfile) {
  return request<ClaudeIdentityPreview>({
    url: '/api/admin/settings/client-profiles/claude/preview',
    method: 'POST',
    data: { configuration },
    silent: true,
  })
}

export interface AccountClientProfile {
  revision: number
  configuration: ClaudeClientProfile | null
}

export function getAccountClientProfile(accountId: string) {
  return request<AccountClientProfile>({
    url: '/api/admin/accounts/client-profile',
    method: 'GET',
    params: { accountId },
    silent: true,
  })
}

export function saveAccountClientProfile(accountId: string, expectedRevision: number, configuration: ClaudeClientProfile | null) {
  return request<AccountClientProfile>({
    url: '/api/admin/accounts/client-profile',
    method: 'POST',
    data: { accountId, expectedRevision, configuration },
    silent: true,
  })
}
