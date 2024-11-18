use crate::gateway_security::google::GoogleIdentityProvider;
use crate::gateway_security::IdentityProvider;
use openidconnect::{ClientId, ClientSecret, IssuerUrl, RedirectUrl, Scope};
use std::fmt::Display;

// SecurityScheme shouldn't have Serialize or Deserialize
#[derive(Debug, Clone)]
pub struct SecurityScheme {
    provider_name: ProviderName,
    scheme_identifier: SecuritySchemeIdentifier,
    client_id: ClientId,
    client_secret: ClientSecret, // secret type macros and therefore already redacted
    redirect_url: RedirectUrl,
    scopes: Vec<Scope>,
    issuer_url: IssuerUrl,
}

impl PartialEq for SecurityScheme {
    fn eq(&self, other: &Self) -> bool {
        self.provider_name == other.provider_name
            && self.scheme_identifier == other.scheme_identifier
            && self.client_id == other.client_id
            && self.client_secret.secret() == other.client_secret.secret()
            && self.redirect_url == other.redirect_url
            && self.scopes == other.scopes
            && self.issuer_url == other.issuer_url
    }
}

impl SecurityScheme {
    pub fn issue_url(&self) -> IssuerUrl {
        self.issuer_url.clone()
    }

    pub fn provider_name(&self) -> ProviderName {
        self.provider_name.clone()
    }

    pub fn provider(&self) -> impl IdentityProvider {
        if self.provider_name.0 == "google" {
            GoogleIdentityProvider::default()
        } else {
            panic!("Make it ADT"); // TODO
        }
    }

    pub fn scheme_identifier(&self) -> SecuritySchemeIdentifier {
        self.scheme_identifier.clone()
    }

    pub fn scopes(&self) -> Vec<Scope> {
        self.scopes.clone()
    }
}


#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ProviderName(String);

impl Display for ProviderName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl ProviderName {
    pub fn new(value: String) -> ProviderName {
        ProviderName(value)
    }
}

#[derive(Debug, Clone, PartialEq, Hash, Eq)]
pub struct SecuritySchemeIdentifier(String);

impl SecuritySchemeIdentifier {
    pub fn new(value: String) -> Self {
        SecuritySchemeIdentifier(value)
    }
}

impl Display for SecuritySchemeIdentifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl SecurityScheme {
    pub fn issuer_url(&self) -> &IssuerUrl {
        &self.issuer_url
    }

    pub fn redirect_url(&self) -> RedirectUrl {
        self.redirect_url.clone()
    }

    pub fn client_id(&self) -> &ClientId {
        &self.client_id
    }

    pub fn client_secret(&self) -> &ClientSecret {
        &self.client_secret
    }

    fn from(
        provider_name: ProviderName,
        scheme_id: &str,
        client_id: &str,
        client_secret: &str,
        redirect_uri: &str,
        scope: Vec<&str>,
        issuer_url: IssuerUrl,
    ) -> Result<SecurityScheme, String> {
        let redirect_url = RedirectUrl::new(redirect_uri.to_string())
            .map_err(|err| format!("Invalid redirect URL, {} {}", redirect_uri, err))?;

        let scheme_identifier = if !scheme_id.is_empty() {
            SecuritySchemeIdentifier(scheme_id.to_string())
        } else {
            return Err("Invalid scheme identifier".to_string());
        };

        let client_id = if !client_id.is_empty() {
            ClientId::new(client_id.to_string())
        } else {
            return Err("Invalid client ID".to_string());
        };

        let client_secret = if !client_secret.is_empty() {
            ClientSecret::new(client_secret.to_string())
        } else {
            return Err("Invalid client secret".to_string());
        };

        let scopes = scope.iter().map(|s| Scope::new(s.to_string())).collect();

        Ok(SecurityScheme {
            provider_name,
            scheme_identifier,
            client_id,
            client_secret,
            redirect_url,
            scopes,
            issuer_url,
        })
    }

    pub fn google_with_default_scope(
        scheme_id: &str,
        client_id: &str,
        client_secret: &str,
        redirect_uri: &str,
    ) -> Result<SecurityScheme, String> {
        let issuer_url =
            IssuerUrl::new("https://accounts.google.com".to_string()).map_err(|err| {
                format!("Invalid Issuer URL for Google, {}", err) // shouldn't happen
            })?;

        Self::from(
            ProviderName("google".to_string()),
            scheme_id,
            client_id,
            client_secret,
            redirect_uri,
            vec!["openid", "email", "profile"],
            issuer_url,
        )
    }
}

impl TryFrom<golem_api_grpc::proto::golem::apidefinition::SecurityScheme> for SecurityScheme {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::apidefinition::SecurityScheme,
    ) -> Result<Self, Self::Error> {
        let client_id = ClientId::new(value.client_id);
        let client_secret = ClientSecret::new(value.client_secret);
        let issuer_url =
            IssuerUrl::new(value.issue_url).map_err(|err| format!("Invalid Issuer. {}", err))?;

        let provider_name = ProviderName::new(value.provider_name);
        let scheme_identifier = SecuritySchemeIdentifier::new(value.scheme_identifier);
        let redirect_url = RedirectUrl::new(value.redirect_url)
            .map_err(|err| format!("Invalid RedirectURL. {}", err))?;

        let scopes: Vec<Scope> = value.scopes.iter().map(|x| Scope::new(x.clone())).collect();

        Ok(SecurityScheme {
            client_secret,
            client_id,
            issuer_url,
            provider_name,
            scheme_identifier,
            redirect_url,
            scopes,
        })
    }
}

impl From<SecurityScheme> for golem_api_grpc::proto::golem::apidefinition::SecurityScheme {
    fn from(value: SecurityScheme) -> Self {
        golem_api_grpc::proto::golem::apidefinition::SecurityScheme {
            provider_name: value.provider_name.to_string(),
            scheme_identifier: value.scheme_identifier.to_string(),
            client_id: value.client_id.to_string(),
            client_secret: value.client_secret.secret().clone(),
            redirect_url: value.redirect_url.to_string(),
            scopes: value.scopes.iter().map(|x| x.to_string()).collect(),
            issue_url: value.issuer_url.to_string(),
        }
    }
}
