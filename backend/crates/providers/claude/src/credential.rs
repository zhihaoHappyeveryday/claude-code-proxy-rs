//! Claude Code 凭据；普通管理投影只包含类型与有效期。
use chrono::{DateTime, Utc};
use gateway_admin::{
    model::provider_credentials::ProviderDocument, ports::provider::ProviderAdminError,
};
use gateway_core::account::OpaqueProviderData;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum Kind {
    #[serde(rename = "oauth")]
    OAuth,
    SetupToken,
}
impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OAuth => "oauth",
            Self::SetupToken => "setup_token",
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Import {
    pub name: String,
    pub authentication_kind: Kind,
    pub access_token: String,
    pub expires_at: Option<DateTime<Utc>>,
}
impl Import {
    pub fn validate(&self) -> Result<(), ProviderAdminError> {
        if self.name.trim().is_empty()
            || self.name.len() > 512
            || self.name.chars().any(char::is_control)
            || self.authentication_kind != Kind::SetupToken
        {
            return Err(super::invalid());
        }
        self.credential().validate()
    }
    pub fn credential(&self) -> Credential {
        Credential {
            authentication_kind: self.authentication_kind,
            access_token: self.access_token.clone(),
            refresh_token: None,
            expires_at: self.expires_at,
            scope: "user:inference".into(),
            account_uuid: None,
            org_uuid: None,
            email: None,
        }
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Credential {
    pub authentication_kind: Kind,
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub scope: String,
    pub account_uuid: Option<String>,
    pub org_uuid: Option<String>,
    pub email: Option<String>,
}
impl Credential {
    pub fn parse(value: &Map<String, Value>) -> Result<Self, ProviderAdminError> {
        let material: Self =
            serde_json::from_value(Value::Object(value.clone())).map_err(|_| super::invalid())?;
        material.validate()?;
        Ok(material)
    }
    pub fn validate(&self) -> Result<(), ProviderAdminError> {
        if !valid_token(&self.access_token)
            || self
                .refresh_token
                .as_deref()
                .is_some_and(|v| !valid_token(v))
            || (self.authentication_kind == Kind::SetupToken && self.refresh_token.is_some())
        {
            return Err(super::invalid());
        }
        Ok(())
    }
    pub fn document(&self) -> Result<ProviderDocument, ProviderAdminError> {
        let Value::Object(value) = serde_json::to_value(self).map_err(|_| super::invalid())? else {
            return Err(super::invalid());
        };
        Ok(ProviderDocument::new(OpaqueProviderData::new(value)))
    }
    pub fn configuration(&self) -> ProviderDocument {
        ProviderDocument::new(OpaqueProviderData::new(Map::from_iter([
            (
                "authentication_kind".into(),
                self.authentication_kind.as_str().into(),
            ),
            ("expires_at".into(), serde_json::json!(self.expires_at)),
            ("refreshable".into(), self.refresh_token.is_some().into()),
        ])))
    }
}
fn valid_token(value: &str) -> bool {
    !value.is_empty() && value.len() <= 16 * 1024 && value.bytes().all(|b| b.is_ascii_graphic())
}
