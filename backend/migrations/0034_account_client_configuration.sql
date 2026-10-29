alter table provider_accounts
    add column provider_configuration jsonb,
    add column configuration_revision bigint not null default 1,
    add constraint provider_accounts_configuration_object
        check (provider_configuration is null or
               (jsonb_typeof(provider_configuration) = 'object' and
                octet_length(provider_configuration::text) <= 8192)),
    add constraint provider_accounts_configuration_revision_positive
        check (configuration_revision > 0);

-- 未发布的开发数据一次性转换；运行时代码只接受统一客户端配置结构。
update runtime_settings
set provider_request_profiles_json = jsonb_set(provider_request_profiles_json, '{claude}',
    jsonb_build_object('schemaVersion', 1,
        'httpIdentity', provider_request_profiles_json -> 'claude',
        'tls', jsonb_build_object('mode', 'standard')))
where jsonb_typeof(provider_request_profiles_json -> 'claude') = 'object'
  and not (provider_request_profiles_json -> 'claude' ? 'schemaVersion');

update client_api_keys
set provider_request_profiles_json = jsonb_set(provider_request_profiles_json, '{claude}',
    jsonb_build_object('schemaVersion', 1,
        'httpIdentity', provider_request_profiles_json -> 'claude',
        'tls', jsonb_build_object('mode', 'standard')))
where jsonb_typeof(provider_request_profiles_json -> 'claude') = 'object'
  and not (provider_request_profiles_json -> 'claude' ? 'schemaVersion');
