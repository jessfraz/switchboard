use clap::{Args, Subcommand, ValueEnum};
use reqwest::Method;
use serde_json::{json, Value};

use crate::{
    client::{AuthMode, RequestBody, RequestSpec},
    Error, MomenceClient, ResolvedContext, Result,
};

#[derive(Debug, Args)]
pub(crate) struct AuthCommand {
    #[command(subcommand)]
    pub(crate) command: AuthSubcommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum AuthSubcommand {
    #[command(name = "authorize-url")]
    AuthorizeUrl(AuthAuthorizeUrlArgs),
    #[command(name = "login-password")]
    LoginPassword(AuthLoginPasswordArgs),
    #[command(name = "exchange-code")]
    ExchangeCode(AuthExchangeCodeArgs),
    Refresh(AuthRefreshArgs),
    Profile,
    Logout,
}

#[derive(Debug, Args)]
pub(crate) struct AuthAuthorizeUrlArgs {
    #[arg(long, value_name = "URL")]
    redirect_uri: String,

    #[arg(long, value_enum)]
    prompt: Option<AuthPrompt>,

    #[arg(long)]
    state: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct AuthLoginPasswordArgs {
    #[arg(long)]
    username: String,

    #[arg(long)]
    password: String,

    #[arg(long)]
    no_store: bool,
}

#[derive(Debug, Args)]
pub(crate) struct AuthExchangeCodeArgs {
    #[arg(long)]
    code: String,

    #[arg(long, value_name = "URL")]
    redirect_uri: String,

    #[arg(long)]
    no_store: bool,
}

#[derive(Debug, Args)]
pub(crate) struct AuthRefreshArgs {
    #[arg(long)]
    refresh_token: Option<String>,

    #[arg(long)]
    no_store: bool,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum AuthPrompt {
    #[value(name = "login")]
    Login,
    #[value(name = "sign-up")]
    SignUp,
    #[value(name = "none")]
    None,
}

impl AuthPrompt {
    fn as_api_value(self) -> &'static str {
        match self {
            Self::Login => "login",
            Self::SignUp => "sign-up",
            Self::None => "none",
        }
    }
}

pub(crate) fn run_auth(
    command: AuthSubcommand,
    client: &MomenceClient,
    context: &mut ResolvedContext,
) -> Result<Value> {
    match command {
        AuthSubcommand::AuthorizeUrl(args) => {
            let client_id = context.require_client_id()?;
            let mut url = client.build_url("/api/v2/auth/authorize", &[])?;
            {
                let mut pairs = url.query_pairs_mut();
                pairs.append_pair("client_id", client_id);
                pairs.append_pair("redirect_uri", &args.redirect_uri);
                pairs.append_pair("response_type", "code");
                pairs.append_pair("scope", "public-api-v2");
                if let Some(prompt) = args.prompt {
                    pairs.append_pair("prompt", prompt.as_api_value());
                }
                if let Some(state) = args.state.as_ref() {
                    pairs.append_pair("state", state);
                }
            }

            Ok(json!({
                "authorize_url": url.as_str(),
            }))
        }
        AuthSubcommand::LoginPassword(args) => {
            let (client_id, client_secret) = context.require_client_credentials()?;
            let response = client.execute(RequestSpec {
                method: Method::POST,
                path: "/api/v2/auth/token".into(),
                query: Vec::new(),
                body: RequestBody::Form(vec![
                    ("grant_type".into(), "password".into()),
                    ("username".into(), args.username),
                    ("password".into(), args.password),
                ]),
                auth: AuthMode::Basic {
                    username: client_id.to_owned(),
                    password: client_secret.to_owned(),
                },
            })?;

            if !args.no_store {
                context.store_tokens_from_response(&response)?;
            }

            Ok(response)
        }
        AuthSubcommand::ExchangeCode(args) => {
            let (client_id, client_secret) = context.require_client_credentials()?;
            let response = client.execute(RequestSpec {
                method: Method::POST,
                path: "/api/v2/auth/token".into(),
                query: Vec::new(),
                body: RequestBody::Form(vec![
                    ("grant_type".into(), "authorization_code".into()),
                    ("code".into(), args.code),
                    ("redirect_uri".into(), args.redirect_uri),
                ]),
                auth: AuthMode::Basic {
                    username: client_id.to_owned(),
                    password: client_secret.to_owned(),
                },
            })?;

            if !args.no_store {
                context.store_tokens_from_response(&response)?;
            }

            Ok(response)
        }
        AuthSubcommand::Refresh(args) => {
            let (client_id, client_secret) = context.require_client_credentials()?;
            let refresh_token = args
                .refresh_token
                .or_else(|| context.refresh_token.clone())
                .ok_or_else(|| Error::Config("missing refresh token, pass --refresh-token or login first".into()))?;

            let response = client.execute(RequestSpec {
                method: Method::POST,
                path: "/api/v2/auth/token".into(),
                query: Vec::new(),
                body: RequestBody::Form(vec![
                    ("grant_type".into(), "refresh_token".into()),
                    ("refresh_token".into(), refresh_token),
                ]),
                auth: AuthMode::Basic {
                    username: client_id.to_owned(),
                    password: client_secret.to_owned(),
                },
            })?;

            if !args.no_store {
                context.store_tokens_from_response(&response)?;
            }

            Ok(response)
        }
        AuthSubcommand::Profile => client.execute(RequestSpec {
            method: Method::GET,
            path: "/api/v2/auth/profile".into(),
            query: Vec::new(),
            body: RequestBody::None,
            auth: AuthMode::Bearer(context.require_access_token()?.to_owned()),
        }),
        AuthSubcommand::Logout => {
            let response = client.execute(RequestSpec {
                method: Method::POST,
                path: "/api/v2/auth/logout".into(),
                query: Vec::new(),
                body: RequestBody::None,
                auth: AuthMode::Bearer(context.require_access_token()?.to_owned()),
            })?;
            context.clear_tokens()?;
            Ok(response)
        }
    }
}
