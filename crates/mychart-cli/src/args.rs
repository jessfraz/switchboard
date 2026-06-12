use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use crate::{
    commands::{
        ApiCommand, AppointmentsCommand, AuthAuthorizeOptions, AuthCommand, ClaimsCommand, ConnectCommand, LabsCommand,
        MedsCommand, NotesCommand, PackCommand, PortalCommand, TimelineCommand,
    },
    state::{
        ENV_MYCHART_ACCESS_TOKEN, ENV_MYCHART_ACCOUNT, ENV_MYCHART_BASE_URL, ENV_MYCHART_CLIENT_ID,
        ENV_MYCHART_CLIENT_SECRET, ENV_MYCHART_CONFIG, ENV_MYCHART_DEBUG_AUTH, ENV_MYCHART_PORTAL_BASE_URL,
        ENV_MYCHART_REDIRECT_URI, ENV_MYCHART_REFRESH_TOKEN, ENV_MYCHART_USERNAME,
    },
};

const AFTER_HELP: &str = concat!(
    "Examples:\n",
    "  mychart login ucla\n",
    "  mychart connect search ucla\n",
    "  mychart connect ucla\n",
    "  mychart connect epic-sandbox\n",
    "  mychart connect ucla medical center\n",
    "  mychart timeline --limit 25\n",
    "  mychart labs a1c ferritin tsh --spark\n",
    "  mychart appointments upcoming --limit 5\n",
    "  mychart appointments find derm --next 30d\n",
    "  mychart meds reconcile --all-providers\n",
    "  mychart notes search --query migraine\n",
    "  mychart notes get note-123\n",
    "  mychart claims audit --since 1y\n",
    "  mychart pack doctor\n",
    "  mychart api resources --details\n",
    "  mychart api appointment search --patient 123 --date ge2026-03-01 --status booked\n",
    "  mychart api observation get obs-123\n",
    "  mychart finish '<auth-code>'  # fallback if the browser cannot reach the local login bridge\n",
    "  mychart portal auth login-password --portal-base-url https://my.uclahealth.org/MyChart \\\n",
    "    --username you@example.com --password 'super-secret'\n",
    "\n",
    "This CLI targets the patient-facing Epic SMART on FHIR surface first, with a resource-driven command grammar\n",
    "that is pleasant for both humans and switchboard to synthesize. The legacy portal session commands stay under\n",
    "`mychart portal ...` for the weird corners Epic still refuses to expose cleanly.\n",
);

#[derive(Debug, Parser)]
#[command(
    name = "mychart",
    version,
    about = "CLI for patient-facing Epic SMART on FHIR workflows, provider discovery, and MyChart portal fallbacks",
    disable_help_subcommand = true,
    after_help = AFTER_HELP
)]
pub(crate) struct Cli {
    #[command(flatten)]
    pub(crate) global: GlobalArgs,

    #[command(subcommand)]
    pub(crate) command: Commands,
}

#[derive(Debug, Args)]
pub(crate) struct GlobalArgs {
    #[arg(long, global = true, env = ENV_MYCHART_CONFIG, value_name = "PATH")]
    pub(crate) config: Option<PathBuf>,

    #[arg(long, global = true, env = ENV_MYCHART_ACCOUNT, value_name = "ACCOUNT")]
    pub(crate) account: Option<String>,

    #[arg(long, global = true, env = ENV_MYCHART_BASE_URL, value_name = "URL")]
    pub(crate) base_url: Option<String>,

    #[arg(long, global = true, env = ENV_MYCHART_PORTAL_BASE_URL, value_name = "URL")]
    pub(crate) portal_base_url: Option<String>,

    #[arg(long, global = true, env = ENV_MYCHART_CLIENT_ID, value_name = "CLIENT_ID")]
    pub(crate) client_id: Option<String>,

    #[arg(long, global = true, env = ENV_MYCHART_CLIENT_SECRET, value_name = "CLIENT_SECRET")]
    pub(crate) client_secret: Option<String>,

    #[arg(long, global = true, env = ENV_MYCHART_REDIRECT_URI, value_name = "URL")]
    pub(crate) redirect_uri: Option<String>,

    #[arg(long, global = true, env = ENV_MYCHART_ACCESS_TOKEN, value_name = "TOKEN")]
    pub(crate) access_token: Option<String>,

    #[arg(long, global = true, env = ENV_MYCHART_REFRESH_TOKEN, value_name = "TOKEN")]
    pub(crate) refresh_token: Option<String>,

    #[arg(long, global = true, env = ENV_MYCHART_USERNAME, value_name = "USERNAME")]
    pub(crate) username: Option<String>,

    #[arg(long, global = true, env = ENV_MYCHART_DEBUG_AUTH)]
    pub(crate) debug_auth: bool,

    #[arg(long, global = true)]
    pub(crate) compact: bool,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Commands {
    Login(LoginCommand),
    Finish(FinishCommand),
    Connect(ConnectCommand),
    Auth(AuthCommand),
    Api(ApiCommand),
    Timeline(TimelineCommand),
    Labs(LabsCommand),
    Notes(NotesCommand),
    Meds(MedsCommand),
    Appointments(AppointmentsCommand),
    Claims(ClaimsCommand),
    Pack(PackCommand),
    Portal(PortalCommand),
}

#[derive(Debug, Args)]
pub(crate) struct LoginCommand {
    #[arg(value_name = "ACCOUNT_OR_PROVIDER")]
    pub(crate) target: Vec<String>,

    #[command(flatten)]
    pub(crate) options: AuthAuthorizeOptions,

    #[arg(long, default_value_t = 300)]
    pub(crate) timeout_seconds: u64,

    #[arg(long)]
    pub(crate) no_open: bool,

    #[arg(long)]
    pub(crate) dynamic_client: bool,

    #[arg(long, value_name = "URL")]
    pub(crate) callback_url: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct FinishCommand {
    pub(crate) callback_input: String,

    #[arg(long)]
    pub(crate) no_store: bool,
}
