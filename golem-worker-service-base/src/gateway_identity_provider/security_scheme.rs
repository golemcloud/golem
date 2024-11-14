use openidconnect::{ClientId, ClientSecret, IssuerUrl, RedirectUrl, Scope};

#[derive(Debug, Clone)]
pub struct SecurityScheme {
    provider_name: ProviderName,
    scheme_identifier: SchemeIdentifier,
    client_id: ClientId,
    client_secret: ClientSecret, // secret type macros and therefore already redacted
    redirect_url: RedirectUrl,
    scopes: Vec<Scope>,
    issuer_url: IssuerUrl
}

#[derive(Debug, Clone)]
pub struct ProviderName(String);

#[derive(Debug, Clone)]
pub struct SchemeIdentifier(String);

impl SecurityScheme {
    pub fn issuer_url(&self) -> &IssuerUrl {
        &self.issuer_url
    }

    pub fn redirect_url(&self) -> &RedirectUrl {
        &self.redirect_url
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
        issuer_url: IssuerUrl
    ) -> Result<SecurityScheme, String> {
        let redirect_url = RedirectUrl::new(redirect_uri.to_string()).map_err(
            |err| {
                format!("Invalid redirect URL, {} {}", redirect_uri, err)
            }
        )?;

        let scheme_identifier = if !scheme_id.is_empty() {
            SchemeIdentifier(scheme_id.to_string())
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

        let scopes =
            scope.iter().map(|s| Scope::new(s.to_string())).collect();

        Ok(SecurityScheme {
            provider_name,
            scheme_identifier,
            client_id,
            client_secret,
            redirect_url,
            scopes,
            issuer_url
        })
    }

    pub fn google_default_scope(scheme_id: &str, client_id: &str, client_secret: &str, redirect_uri: &str, scope: &str) -> Result<SecurityScheme, String> {
        let issuer_url =  IssuerUrl::new("https://accounts.google.com".to_string()).map_err(|err| {
            format!("Invalid Issuer URL for Google, {}", err) // shouldn't happen
        })?;

        Self::from(
            ProviderName("google".to_string()),
            scheme_id,
            client_id,
            client_secret,
            redirect_uri,
            vec!["openid", "email", "profile"],
            issuer_url
        )
    }
}