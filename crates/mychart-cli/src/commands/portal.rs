use clap::{Args, Subcommand};
use reqwest::Method;
use serde_json::{json, Value};

use crate::{
    client::{cookie_names, RequestBody, RequestSpec},
    ensure_portal_success_status, extract_login_error, extract_verification_token, is_login_page, login_error_message,
    looks_like_verification_challenge, parse_portal_response_body, portal_client,
    state::ResolvedContext,
    summarize_page, Error, Result,
};

#[derive(Debug, Args)]
pub(crate) struct PortalCommand {
    #[command(subcommand)]
    pub(crate) command: PortalSubcommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum PortalSubcommand {
    Auth(PortalAuthCommand),
    Request(PortalRequestCommand),
}

#[derive(Debug, Args)]
pub(crate) struct PortalAuthCommand {
    #[command(subcommand)]
    pub(crate) command: PortalAuthSubcommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum PortalAuthSubcommand {
    #[command(name = "login-password")]
    LoginPassword(PortalAuthLoginPasswordArgs),
    Status,
    Logout,
}

#[derive(Debug, Args)]
pub(crate) struct PortalAuthLoginPasswordArgs {
    #[arg(long)]
    username: Option<String>,

    #[arg(long)]
    password: String,

    #[arg(long)]
    no_store: bool,
}

#[derive(Debug, Args)]
pub(crate) struct PortalRequestCommand {
    #[command(subcommand)]
    pub(crate) command: PortalRequestSubcommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum PortalRequestSubcommand {
    Get(PortalRequestGetArgs),
}

#[derive(Debug, Args)]
pub(crate) struct PortalRequestGetArgs {
    path: String,

    #[arg(long = "query", value_parser = crate::parse_key_value, value_name = "KEY=VALUE")]
    query: Vec<(String, String)>,

    #[arg(long = "no-follow-redirects")]
    no_follow_redirects: bool,
}

pub(crate) fn run_portal(command: PortalSubcommand, context: &mut ResolvedContext) -> Result<Value> {
    match command {
        PortalSubcommand::Auth(command) => run_portal_auth(command.command, context),
        PortalSubcommand::Request(command) => run_portal_request(command.command, context),
    }
}

fn run_portal_auth(command: PortalAuthSubcommand, context: &mut ResolvedContext) -> Result<Value> {
    let portal_base_url = context.require_portal_base_url()?;
    let client = portal_client(&portal_base_url)?;

    match command {
        PortalAuthSubcommand::LoginPassword(args) => {
            let username = context.require_username(args.username)?;
            let mut cookies = context.cookies.clone();

            let login_page = client.execute(
                RequestSpec {
                    method: Method::GET,
                    path: "/Authentication/Login".into(),
                    query: Vec::new(),
                    body: RequestBody::None,
                },
                &mut cookies,
                true,
            )?;
            ensure_portal_success_status(&login_page)?;

            let csrf_token = extract_verification_token(&login_page.body_text).ok_or_else(|| Error::Auth {
                message: "failed to locate the MyChart login CSRF token".into(),
                details: json!({
                    "final_url": login_page.final_url.as_str(),
                    "page": summarize_page(&login_page.final_url, &login_page.body_text),
                }),
            })?;

            let login_payload = json!({
                "Type": "StandardLogin",
                "Credentials": {
                    "LoginIdentifier": crate::base64_encode(username.as_bytes()),
                    "Password": crate::base64_encode(args.password.as_bytes()),
                }
            })
            .to_string();

            let response = client.execute(
                RequestSpec {
                    method: Method::POST,
                    path: "/Authentication/Login/DoLogin".into(),
                    query: Vec::new(),
                    body: RequestBody::Form(vec![
                        ("__RequestVerificationToken".into(), csrf_token),
                        ("LoginInfo".into(), login_payload),
                        ("DeviceId".into(), context.device_id.clone()),
                    ]),
                },
                &mut cookies,
                true,
            )?;

            if let Some(error_code) = extract_login_error(&response.final_url) {
                return Err(Error::Auth {
                    message: login_error_message(&error_code),
                    details: json!({
                        "error_code": error_code,
                        "final_url": response.final_url.as_str(),
                        "page": summarize_page(&response.final_url, &response.body_text),
                    }),
                });
            }

            ensure_portal_success_status(&response)?;

            let page = summarize_page(&response.final_url, &response.body_text);
            let verification_required = looks_like_verification_challenge(&response.final_url, &response.body_text);

            if is_login_page(&response.body_text) && !verification_required {
                return Err(Error::Auth {
                    message: "MyChart returned to the login page without establishing a portal session".into(),
                    details: json!({
                        "final_url": response.final_url.as_str(),
                        "page": page,
                    }),
                });
            }

            if !args.no_store {
                context.update_cookies(cookies.clone());
                context.store_portal_session(portal_base_url.clone(), Some(username.clone()))?;
            }

            Ok(json!({
                "status": if verification_required { "verification_required" } else { "authenticated" },
                "username": username,
                "portal_base_url": portal_base_url,
                "final_url": response.final_url.as_str(),
                "next_url": if verification_required { Value::String(response.final_url.to_string()) } else { Value::Null },
                "redirect_chain": response.redirect_chain,
                "stored": !args.no_store,
                "cookie_names": cookie_names(&cookies),
                "page": page,
            }))
        }
        PortalAuthSubcommand::Status => {
            if !context.has_portal_session() {
                return Ok(json!({
                    "status": "ok",
                    "authenticated": false,
                    "reason": "no_stored_session",
                    "portal_base_url": portal_base_url,
                    "username": context.username,
                }));
            }

            let mut cookies = context.cookies.clone();
            let response = client.execute(
                RequestSpec {
                    method: Method::GET,
                    path: "/inside.asp".into(),
                    query: Vec::new(),
                    body: RequestBody::None,
                },
                &mut cookies,
                true,
            )?;

            ensure_portal_success_status(&response)?;

            let page = summarize_page(&response.final_url, &response.body_text);
            let authenticated = !is_login_page(&response.body_text);

            if authenticated {
                context.update_cookies(cookies);
                context.store_portal_session(portal_base_url.clone(), None)?;
            } else {
                context.clear_portal_session()?;
            }

            Ok(json!({
                "status": "ok",
                "authenticated": authenticated,
                "portal_base_url": portal_base_url,
                "username": context.username,
                "final_url": response.final_url.as_str(),
                "redirect_chain": response.redirect_chain,
                "cookie_names": if authenticated { context.cookie_names() } else { Vec::<String>::new() },
                "page": page,
            }))
        }
        PortalAuthSubcommand::Logout => {
            if context.has_portal_session() {
                let mut cookies = context.cookies.clone();
                let response = client.execute(
                    RequestSpec {
                        method: Method::GET,
                        path: "/Home/LogOut".into(),
                        query: Vec::new(),
                        body: RequestBody::None,
                    },
                    &mut cookies,
                    true,
                )?;
                ensure_portal_success_status(&response)?;
            }

            context.clear_portal_session()?;
            Ok(json!({
                "status": "logged_out",
                "portal_base_url": portal_base_url,
                "username": context.username,
            }))
        }
    }
}

fn run_portal_request(command: PortalRequestSubcommand, context: &mut ResolvedContext) -> Result<Value> {
    context.require_portal_session()?;
    let portal_base_url = context.require_portal_base_url()?;
    let client = portal_client(&portal_base_url)?;

    let (method, path, query, body, follow_redirects) = match command {
        PortalRequestSubcommand::Get(args) => (
            Method::GET,
            args.path,
            args.query,
            RequestBody::None,
            !args.no_follow_redirects,
        ),
    };

    let mut cookies = context.cookies.clone();
    let response = client.execute(
        RequestSpec {
            method: method.clone(),
            path,
            query,
            body,
        },
        &mut cookies,
        follow_redirects,
    )?;

    ensure_portal_success_status(&response)?;

    if is_login_page(&response.body_text) {
        context.clear_portal_session()?;
        return Err(Error::Auth {
            message: "stored MyChart portal session is not authenticated anymore, run mychart portal auth login-password again"
                .into(),
            details: json!({
                "final_url": response.final_url.as_str(),
                "page": summarize_page(&response.final_url, &response.body_text),
            }),
        });
    }

    context.update_cookies(cookies);
    context.store_portal_session(portal_base_url, None)?;

    Ok(json!({
        "status": "ok",
        "request": {
            "method": method.as_str(),
        },
        "response": {
            "status_code": response.status_code,
            "final_url": response.final_url.as_str(),
            "location": response.location,
            "content_type": response.content_type,
            "redirect_chain": response.redirect_chain,
        },
        "page": summarize_page(&response.final_url, &response.body_text),
        "body": parse_portal_response_body(&response),
    }))
}
