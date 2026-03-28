use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    process::ExitCode,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use clap_complete::{
    generate_to,
    shells::{Bash, Elvish, Fish, PowerShell, Zsh},
};
use clap_mangen::Man;
use serde::Serialize;
use switchboard_cli::{
    catalog::{ToolCatalogDetail, ToolCatalogStatus},
    command as switchboard_command,
};
use switchboard_core::{ProviderKind, RegisteredTool, ToolExecutionSupport, ToolSurface, ToolUndoSupport};
use switchboard_providers::default_registry;

const SITE_URL: &str = "https://jessfraz.github.io/switchboard/";
const REPO_URL: &str = "https://github.com/jessfraz/switchboard";

#[derive(Parser)]
#[command(name = "switchboard-docs-gen")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Generate,
    Check,
    Site {
        #[arg(long, value_name = "PATH")]
        output_dir: PathBuf,
    },
}

#[derive(Clone, Debug, Serialize)]
struct CatalogSnapshot {
    stats: CatalogStats,
    providers: Vec<ProviderSnapshot>,
    tools: Vec<ToolSnapshot>,
}

#[derive(Clone, Debug, Serialize)]
struct CatalogStats {
    tool_count: usize,
    curated_count: usize,
    raw_count: usize,
    stable_count: usize,
    planning_only_count: usize,
    undoable_count: usize,
}

#[derive(Clone, Debug, Serialize)]
struct ProviderSnapshot {
    provider: ProviderKind,
    tool_count: usize,
    curated_count: usize,
    raw_count: usize,
    stable_count: usize,
    planning_only_count: usize,
    undoable_count: usize,
    doc_path: String,
}

#[derive(Clone, Debug, Serialize)]
struct ToolSnapshot {
    #[serde(flatten)]
    detail: ToolCatalogDetail,
    doc_path: String,
}

#[derive(Clone, Debug)]
struct RenderedSite {
    files: BTreeMap<PathBuf, String>,
}

impl RenderedSite {
    fn new() -> Self {
        Self { files: BTreeMap::new() }
    }

    fn insert(&mut self, path: impl Into<PathBuf>, contents: String) {
        self.files.insert(path.into(), contents);
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let snapshot = load_snapshot()?;
    match cli.command {
        Command::Generate => {
            let rendered = render_docs(&snapshot)?;
            write_rendered_docs(&rendered)
        }
        Command::Check => {
            let rendered = render_docs(&snapshot)?;
            check_rendered_docs(&rendered)
        }
        Command::Site { output_dir } => render_deploy_site(&snapshot, &output_dir),
    }
}

fn load_snapshot() -> Result<CatalogSnapshot> {
    let registry = default_registry();
    let tools = registry.list_tools().context("failed to load tool catalog")?;
    Ok(build_snapshot(&tools))
}

fn render_docs(snapshot: &CatalogSnapshot) -> Result<RenderedSite> {
    let workspace_root = workspace_root();
    let mut rendered = RenderedSite::new();
    rendered.insert(
        workspace_root.join("docs/reference/catalog.json"),
        format!(
            "{}\n",
            serde_json::to_string_pretty(&snapshot).context("failed to serialize catalog snapshot")?
        ),
    );
    rendered.insert(
        workspace_root.join("docs/reference/README.md"),
        render_reference_index(snapshot),
    );
    rendered.insert(
        workspace_root.join("docs/reference/support-matrix.md"),
        render_support_matrix(snapshot),
    );
    for provider in &snapshot.providers {
        rendered.insert(
            workspace_root.join(&provider.doc_path),
            render_provider_markdown(provider, &snapshot.tools),
        );
    }

    render_cli_reference(&workspace_root, &mut rendered)?;

    Ok(rendered)
}

fn build_snapshot(tools: &[RegisteredTool]) -> CatalogSnapshot {
    let details = tools
        .iter()
        .map(|tool| ToolSnapshot {
            detail: ToolCatalogDetail::new(tool, &[]),
            doc_path: format!("reference/#tool={}", tool.name),
        })
        .collect::<Vec<_>>();

    let stats = CatalogStats {
        tool_count: tools.len(),
        curated_count: tools.iter().filter(|tool| tool.surface == ToolSurface::Curated).count(),
        raw_count: tools.iter().filter(|tool| tool.surface == ToolSurface::Raw).count(),
        stable_count: tools
            .iter()
            .filter(|tool| status_for(tool) == ToolCatalogStatus::Stable)
            .count(),
        planning_only_count: tools
            .iter()
            .filter(|tool| status_for(tool) == ToolCatalogStatus::PlanningOnly)
            .count(),
        undoable_count: tools
            .iter()
            .filter(|tool| tool.undo_support == ToolUndoSupport::CompensatingAction)
            .count(),
    };

    let providers = [ProviderKind::GitHub, ProviderKind::GoogleWorkspace]
        .into_iter()
        .map(|provider| {
            let provider_tools = tools
                .iter()
                .filter(|tool| tool.provider == provider)
                .collect::<Vec<_>>();
            ProviderSnapshot {
                provider: provider.clone(),
                tool_count: provider_tools.len(),
                curated_count: provider_tools
                    .iter()
                    .filter(|tool| tool.surface == ToolSurface::Curated)
                    .count(),
                raw_count: provider_tools
                    .iter()
                    .filter(|tool| tool.surface == ToolSurface::Raw)
                    .count(),
                stable_count: provider_tools
                    .iter()
                    .filter(|tool| status_for(tool) == ToolCatalogStatus::Stable)
                    .count(),
                planning_only_count: provider_tools
                    .iter()
                    .filter(|tool| status_for(tool) == ToolCatalogStatus::PlanningOnly)
                    .count(),
                undoable_count: provider_tools
                    .iter()
                    .filter(|tool| tool.undo_support == ToolUndoSupport::CompensatingAction)
                    .count(),
                doc_path: format!("docs/reference/providers/{provider}.md"),
            }
        })
        .collect::<Vec<_>>();

    CatalogSnapshot {
        stats,
        providers,
        tools: details,
    }
}

fn render_reference_index(snapshot: &CatalogSnapshot) -> String {
    let mut output = String::new();
    output.push_str("# Reference Catalog\n\n");
    output.push_str("Generated from the live `switchboard` tool registry, committed CLI inventories, and curated provider manifests.\n\n");
    output.push_str("## Snapshot\n\n");
    output.push_str(&format!(
        "- Tools: `{}`\n- Curated: `{}`\n- Raw inventory passthrough: `{}`\n- Planning-only: `{}`\n- Undoable: `{}`\n\n",
        snapshot.stats.tool_count,
        snapshot.stats.curated_count,
        snapshot.stats.raw_count,
        snapshot.stats.planning_only_count,
        snapshot.stats.undoable_count,
    ));
    output.push_str("## Providers\n\n");
    for provider in &snapshot.providers {
        output.push_str(&format!(
            "- [{}]({}) for `{}` tools (`{}` curated, `{}` raw)\n",
            provider.provider,
            relative_to_reference_root(&provider.doc_path),
            provider.tool_count,
            provider.curated_count,
            provider.raw_count
        ));
    }
    output.push_str("\n## Machine-readable Outputs\n\n");
    output.push_str("- [catalog.json](catalog.json)\n");
    output.push_str("- [support-matrix.md](support-matrix.md)\n");
    output.push_str("- [man page](man/switchboard.1)\n");
    output.push_str("- [shell completions](completions/)\n\n");
    output.push_str("## LLM Notes\n\n");
    output.push_str("- Prefer curated tools first, raw tools second.\n");
    output.push_str("- Prefer `--json` for reads and `--draft` before `--apply` for writes.\n");
    output.push_str("- The committed `catalog.json` is the fastest way to inspect the whole surface area without scraping help output.\n");
    output
}

fn render_support_matrix(snapshot: &CatalogSnapshot) -> String {
    let mut output = String::new();
    output.push_str("# Support Matrix\n\n");
    output.push_str("| Provider | Stable curated | Planning-only curated | Raw passthrough | Undoable |\n");
    output.push_str("| --- | ---: | ---: | ---: | ---: |\n");
    for provider in &snapshot.providers {
        output.push_str(&format!(
            "| `{}` | {} | {} | {} | {} |\n",
            provider.provider,
            provider.stable_count,
            provider.planning_only_count,
            provider.raw_count,
            provider.undoable_count
        ));
    }
    output.push_str("\n## Status meanings\n\n");
    output.push_str("- `stable`: curated and executable today.\n");
    output.push_str("- `planning_only`: plans cleanly but does not apply yet.\n");
    output.push_str("- `raw`: generated passthrough to provider-native CLI coverage.\n");
    output
}

fn render_provider_markdown(provider: &ProviderSnapshot, tools: &[ToolSnapshot]) -> String {
    let curated = tools
        .iter()
        .filter(|tool| tool.detail.provider == provider.provider && tool.detail.surface == ToolSurface::Curated)
        .collect::<Vec<_>>();
    let raw = tools
        .iter()
        .filter(|tool| tool.detail.provider == provider.provider && tool.detail.surface == ToolSurface::Raw)
        .collect::<Vec<_>>();

    let mut output = String::new();
    output.push_str(&format!("# `{}` Provider\n\n", provider.provider));
    output.push_str(&format!(
        "`{}` exposes `{}` total tools, `{}` curated and `{}` raw inventory passthrough.\n\n",
        provider.provider, provider.tool_count, provider.curated_count, provider.raw_count
    ));
    output.push_str("## Curated tools\n\n");
    for tool in curated {
        output.push_str(&format!(
            "- `{}` [{}] {}\n",
            tool.detail.name,
            tool.detail.status.as_str(),
            tool.detail.summary
        ));
    }
    output.push_str("\n## Raw surfaces\n\n");
    output.push_str(&format!("- `{}.cli.read`\n", provider.provider));
    output.push_str(&format!("- `{}.cli.write`\n", provider.provider));
    output.push_str(&format!(
        "\n`{}` also ships `{}` more raw command projections. The complete structured surface lives in [catalog.json](../catalog.json) and the deployed reference explorer.\n",
        provider.provider,
        raw.len().saturating_sub(2)
    ));
    output
}

fn render_cli_reference(workspace_root: &Path, rendered: &mut RenderedSite) -> Result<()> {
    let scratch = scratch_dir();
    fs::create_dir_all(&scratch).with_context(|| format!("failed to create {}", scratch.display()))?;
    let completions_dir = scratch.join("completions");
    fs::create_dir_all(&completions_dir).with_context(|| format!("failed to create {}", completions_dir.display()))?;
    let man_dir = workspace_root.join("docs/reference/man");
    let mut command = switchboard_command();
    generate_to(Bash, &mut command, "switchboard", &completions_dir).context("failed to generate bash completion")?;
    let mut command = switchboard_command();
    generate_to(Zsh, &mut command, "switchboard", &completions_dir).context("failed to generate zsh completion")?;
    let mut command = switchboard_command();
    generate_to(Fish, &mut command, "switchboard", &completions_dir).context("failed to generate fish completion")?;
    let mut command = switchboard_command();
    generate_to(PowerShell, &mut command, "switchboard", &completions_dir)
        .context("failed to generate powershell completion")?;
    let mut command = switchboard_command();
    generate_to(Elvish, &mut command, "switchboard", &completions_dir)
        .context("failed to generate elvish completion")?;

    for entry in fs::read_dir(&completions_dir).context("failed to read generated completions")? {
        let entry = entry.context("failed to read completion directory entry")?;
        let path = entry.path();
        if path.is_file() {
            rendered.insert(
                workspace_root
                    .join("docs/reference/completions")
                    .join(entry.file_name()),
                fs::read_to_string(entry.path()).context("failed to load generated completion")?,
            );
        }
    }

    let command = switchboard_command();
    let man = Man::new(command);
    let mut buffer = Vec::new();
    man.render(&mut buffer).context("failed to render man page")?;
    rendered.insert(
        man_dir.join("switchboard.1"),
        String::from_utf8(buffer).context("man page was not valid UTF-8")?,
    );
    let _ = fs::remove_dir_all(&scratch);
    Ok(())
}

#[allow(clippy::uninlined_format_args)]
fn render_reference_html(snapshot: &CatalogSnapshot) -> String {
    let total = snapshot.stats.tool_count;
    let curated = snapshot.stats.curated_count;
    let raw = snapshot.stats.raw_count;
    let planning = snapshot.stats.planning_only_count;
    let site_url = SITE_URL;
    format!(
        r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>switchboard reference</title>
    <style>
      :root {{
        --bg: #f4efe5;
        --panel: rgba(255, 251, 246, 0.92);
        --panel-strong: #fffaf4;
        --ink: #1a1e1a;
        --muted: #5f6256;
        --line: rgba(26, 30, 26, 0.12);
        --accent: #b24a2b;
        --accent-strong: #7f3019;
        --sage: #4b6650;
        --gold: #b28a2b;
        --shadow: 0 18px 44px rgba(60, 37, 15, 0.12);
        --mono: "SFMono-Regular", "SF Mono", "JetBrains Mono", "Cascadia Code", Consolas, monospace;
      }}
      * {{ box-sizing: border-box; }}
      body {{
        margin: 0;
        min-height: 100vh;
        color: var(--ink);
        font-family: "Iowan Old Style", "Palatino Linotype", "Book Antiqua", Palatino, Georgia, serif;
        background:
          radial-gradient(circle at top left, rgba(178, 74, 43, 0.18), transparent 28%),
          radial-gradient(circle at bottom right, rgba(75, 102, 80, 0.2), transparent 32%),
          linear-gradient(180deg, #fbf7f1, var(--bg));
      }}
      .shell {{
        width: min(1380px, 100%);
        margin: 0 auto;
        padding: 2rem;
      }}
      header {{
        display: grid;
        gap: 1rem;
        margin-bottom: 1.5rem;
      }}
      h1 {{
        margin: 0;
        font-size: clamp(2.4rem, 5vw, 4.5rem);
        line-height: 0.95;
        letter-spacing: -0.04em;
      }}
      .lede {{
        color: var(--muted);
        max-width: 60rem;
        font-size: 1.08rem;
        line-height: 1.65;
      }}
      .stats {{
        display: grid;
        grid-template-columns: repeat(auto-fit, minmax(180px, 1fr));
        gap: 0.9rem;
        margin-bottom: 1.5rem;
      }}
      .stat {{
        background: var(--panel);
        border: 1px solid var(--line);
        border-radius: 20px;
        box-shadow: var(--shadow);
        padding: 1rem 1.1rem;
      }}
      .stat-label {{
        display: block;
        margin-bottom: 0.5rem;
        color: var(--muted);
        font-family: var(--mono);
        font-size: 0.76rem;
        text-transform: uppercase;
        letter-spacing: 0.08em;
      }}
      .stat-value {{
        font-size: 2rem;
        line-height: 1;
      }}
      .layout {{
        display: grid;
        grid-template-columns: minmax(320px, 420px) minmax(0, 1fr);
        gap: 1rem;
      }}
      .panel {{
        background: var(--panel);
        border: 1px solid var(--line);
        border-radius: 24px;
        box-shadow: var(--shadow);
      }}
      .filters {{
        padding: 1rem;
        border-bottom: 1px solid var(--line);
        display: grid;
        gap: 0.8rem;
      }}
      label {{
        display: grid;
        gap: 0.4rem;
        font-family: var(--mono);
        color: var(--muted);
        font-size: 0.78rem;
        letter-spacing: 0.06em;
        text-transform: uppercase;
      }}
      input, select {{
        width: 100%;
        border: 1px solid var(--line);
        background: #fff;
        color: var(--ink);
        border-radius: 12px;
        padding: 0.75rem 0.85rem;
        font: inherit;
      }}
      .tool-list {{
        max-height: 70vh;
        overflow: auto;
        padding: 0.5rem;
      }}
      .tool-button {{
        width: 100%;
        text-align: left;
        border: 1px solid transparent;
        background: transparent;
        border-radius: 16px;
        padding: 0.9rem;
        cursor: pointer;
        transition: background 120ms ease, border-color 120ms ease, transform 120ms ease;
      }}
      .tool-button:hover, .tool-button.active {{
        background: rgba(255,255,255,0.82);
        border-color: var(--line);
        transform: translateY(-1px);
      }}
      .tool-name {{
        display: block;
        font-family: var(--mono);
        font-size: 0.84rem;
        margin-bottom: 0.4rem;
      }}
      .tool-summary {{
        display: block;
        color: var(--muted);
        font-size: 0.96rem;
        line-height: 1.45;
      }}
      .badges {{
        display: flex;
        flex-wrap: wrap;
        gap: 0.4rem;
        margin-top: 0.55rem;
      }}
      .badge {{
        border-radius: 999px;
        padding: 0.2rem 0.55rem;
        font-family: var(--mono);
        font-size: 0.7rem;
        letter-spacing: 0.04em;
        text-transform: uppercase;
        background: rgba(255,255,255,0.9);
        border: 1px solid var(--line);
      }}
      .badge.raw {{ color: var(--accent-strong); }}
      .badge.stable {{ color: var(--sage); }}
      .badge.planning_only {{ color: var(--gold); }}
      .detail {{
        padding: 1.3rem;
        display: grid;
        gap: 1rem;
      }}
      .detail h2 {{
        margin: 0;
        font-size: clamp(1.6rem, 3vw, 2.6rem);
        line-height: 1.02;
      }}
      .detail p {{
        margin: 0;
        color: var(--muted);
        line-height: 1.7;
      }}
      .meta-grid {{
        display: grid;
        grid-template-columns: repeat(auto-fit, minmax(180px, 1fr));
        gap: 0.7rem;
      }}
      .meta-card {{
        padding: 0.85rem 0.95rem;
        border: 1px solid var(--line);
        border-radius: 16px;
        background: var(--panel-strong);
      }}
      .meta-key {{
        display: block;
        margin-bottom: 0.45rem;
        color: var(--muted);
        font-family: var(--mono);
        font-size: 0.72rem;
        text-transform: uppercase;
        letter-spacing: 0.08em;
      }}
      .meta-value {{
        font-size: 1rem;
        line-height: 1.4;
        word-break: break-word;
      }}
      .section-title {{
        margin: 0;
        font-family: var(--mono);
        font-size: 0.8rem;
        letter-spacing: 0.08em;
        text-transform: uppercase;
      }}
      ul {{
        margin: 0;
        padding-left: 1.15rem;
        color: var(--muted);
        line-height: 1.7;
      }}
      table {{
        width: 100%;
        border-collapse: collapse;
        font-size: 0.94rem;
      }}
      th, td {{
        padding: 0.7rem 0.75rem;
        border-bottom: 1px solid var(--line);
        text-align: left;
        vertical-align: top;
      }}
      th {{
        font-family: var(--mono);
        font-size: 0.72rem;
        text-transform: uppercase;
        letter-spacing: 0.08em;
        color: var(--muted);
      }}
      pre {{
        margin: 0;
        padding: 1rem;
        overflow: auto;
        border-radius: 18px;
        background: #1d221e;
        color: #f7f3eb;
        font-family: var(--mono);
        font-size: 0.82rem;
        line-height: 1.6;
      }}
      a {{
        color: var(--accent-strong);
        text-decoration-thickness: 2px;
        text-underline-offset: 0.18em;
      }}
      @media (max-width: 980px) {{
        .layout {{
          grid-template-columns: 1fr;
        }}
        .tool-list {{
          max-height: none;
        }}
      }}
    </style>
  </head>
  <body>
    <div class="shell">
      <header>
        <h1>Reference, minus the lies.</h1>
        <p class="lede">
          This catalog is generated from the live Switchboard registry, not hand-maintained docs. It is the fastest way
          to answer “what can this thing actually do right now?” without scraping help output or reading source.
        </p>
      </header>
      <section class="stats">
        <div class="stat"><span class="stat-label">Tools</span><span class="stat-value">{total}</span></div>
        <div class="stat"><span class="stat-label">Curated</span><span class="stat-value">{curated}</span></div>
        <div class="stat"><span class="stat-label">Raw</span><span class="stat-value">{raw}</span></div>
        <div class="stat"><span class="stat-label">Planning Only</span><span class="stat-value">{planning}</span></div>
      </section>
      <section class="layout">
        <aside class="panel">
          <div class="filters">
            <label>
              Search
              <input id="search" type="search" placeholder="tool name, provider, summary">
            </label>
            <label>
              Provider
              <select id="provider-filter">
                <option value="">All providers</option>
                <option value="github">GitHub</option>
                <option value="google">Google</option>
              </select>
            </label>
            <label>
              Status
              <select id="status-filter">
                <option value="">All tracks</option>
                <option value="stable">Stable</option>
                <option value="planning_only">Planning only</option>
                <option value="raw">Raw passthrough</option>
              </select>
            </label>
          </div>
          <div id="tool-list" class="tool-list"></div>
        </aside>
        <main class="panel detail" id="detail"></main>
      </section>
    </div>
    <script>
      const detail = document.getElementById("detail");
      const toolList = document.getElementById("tool-list");
      const search = document.getElementById("search");
      const providerFilter = document.getElementById("provider-filter");
      const statusFilter = document.getElementById("status-filter");

      fetch("./catalog.json")
        .then((response) => response.json())
        .then((catalog) => {{
          const tools = catalog.tools;
          let selected = tools[0]?.name ?? null;

          function badgeClass(value) {{
            return value.replace(/[^a-z_]/g, "_");
          }}

          function renderList() {{
            const q = search.value.trim().toLowerCase();
            const provider = providerFilter.value;
            const status = statusFilter.value;
            const filtered = tools.filter((tool) => {{
              const haystack = `${{tool.name}} ${{tool.provider}} ${{tool.summary}}`.toLowerCase();
              return (!q || haystack.includes(q)) &&
                     (!provider || tool.provider === provider) &&
                     (!status || tool.status === status);
            }});
            if (!filtered.some((tool) => tool.name === selected)) {{
              selected = filtered[0]?.name ?? null;
            }}
            toolList.innerHTML = filtered.map((tool) => {{
              const active = tool.name === selected ? "active" : "";
              return `
                <button class="tool-button ${{active}}" data-tool="${{tool.name}}">
                  <span class="tool-name">${{tool.name}}</span>
                  <span class="tool-summary">${{escapeHtml(tool.summary)}}</span>
                  <span class="badges">
                    <span class="badge ${{badgeClass(tool.status)}}">${{tool.status}}</span>
                    <span class="badge">${{tool.kind}}</span>
                    <span class="badge">${{tool.provider}}</span>
                  </span>
                </button>
              `;
            }}).join("");
            [...toolList.querySelectorAll(".tool-button")].forEach((button) => {{
              button.addEventListener("click", () => {{
                selected = button.dataset.tool;
                location.hash = `tool=${{encodeURIComponent(selected)}}`;
                renderList();
                renderDetail();
              }});
            }});
            renderDetail(filtered);
          }}

          function renderDetail(filtered = tools) {{
            const current = filtered.find((tool) => tool.name === selected) || filtered[0];
            if (!current) {{
              detail.innerHTML = "<p>No tools matched those filters.</p>";
              return;
            }}
            const argumentRows = (current.arguments || []).map((arg) => `
              <tr>
                <td><code>${{arg.name}}</code></td>
                <td><code>${{arg.transport}}</code></td>
                <td><code>${{arg.value_kind}}</code></td>
                <td>${{arg.required ? "yes" : "no"}}</td>
                <td>${{arg.repeated ? "yes" : "no"}}</td>
              </tr>
            `).join("");
            const rows = (current.arguments || []).length
              ? `
                <table>
                  <thead>
                    <tr>
                      <th>Name</th>
                      <th>Transport</th>
                      <th>Type</th>
                      <th>Required</th>
                      <th>Repeated</th>
                    </tr>
                  </thead>
                  <tbody>${{argumentRows}}</tbody>
                </table>
              `
              : "<p>No typed arguments beyond namespace and execution flags.</p>";
            detail.innerHTML = `
              <div>
                <h2>${{current.name}}</h2>
                <p>${{escapeHtml(current.summary)}}</p>
              </div>
              <div class="badges">
                <span class="badge ${{badgeClass(current.status)}}">${{current.status}}</span>
                <span class="badge">${{current.kind}}</span>
                <span class="badge">${{current.provider}}</span>
                <span class="badge">${{current.surface}}</span>
              </div>
              <div class="meta-grid">
                <div class="meta-card"><span class="meta-key">Backend</span><span class="meta-value">${{current.backend}}</span></div>
                <div class="meta-card"><span class="meta-key">Execution</span><span class="meta-value">${{current.execution_support}}</span></div>
                <div class="meta-card"><span class="meta-key">Undo</span><span class="meta-value">${{current.undo_support}}</span></div>
                <div class="meta-card"><span class="meta-key">Aggregate Reads</span><span class="meta-value">${{current.aggregate_read_supported ? "supported" : "single namespace only"}}</span></div>
              </div>
              <div>
                <h3 class="section-title">Arguments</h3>
                ${{rows}}
              </div>
              <div>
                <h3 class="section-title">Notes</h3>
                <ul>${{(current.notes || []).map((note) => `<li>${{escapeHtml(note)}}</li>`).join("")}}</ul>
              </div>
              <div>
                <h3 class="section-title">Examples</h3>
                <pre>${{escapeHtml((current.examples || []).join("\n"))}}</pre>
              </div>
              <p>
                Shareable link:
                <a href="${{SITE_URL}}${{current.doc_path}}">${{SITE_URL}}${{current.doc_path}}</a>
              </p>
            `;
          }}

          function readHash() {{
            const value = new URLSearchParams(location.hash.slice(1)).get("tool");
            if (value) {{
              selected = value;
            }}
          }}

          function escapeHtml(value) {{
            return value
              .replaceAll("&", "&amp;")
              .replaceAll("<", "&lt;")
              .replaceAll(">", "&gt;")
              .replaceAll('"', "&quot;");
          }}

          window.addEventListener("hashchange", () => {{
            readHash();
            renderList();
          }});
          [search, providerFilter, statusFilter].forEach((input) => input.addEventListener("input", renderList));
          readHash();
          renderList();
        }});
      const SITE_URL = "{site_url}";
    </script>
  </body>
</html>
"#,
        total = total,
        curated = curated,
        raw = raw,
        planning = planning,
        site_url = site_url
    )
}

#[allow(clippy::uninlined_format_args)]
fn render_site_index(snapshot: &CatalogSnapshot) -> String {
    let github = snapshot
        .providers
        .iter()
        .find(|provider| provider.provider == ProviderKind::GitHub)
        .expect("github provider should exist");
    let google = snapshot
        .providers
        .iter()
        .find(|provider| provider.provider == ProviderKind::GoogleWorkspace)
        .expect("google provider should exist");

    format!(
        r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>switchboard</title>
    <style>
      :root {{
        --bg: #efe7dc;
        --paper: rgba(255, 250, 244, 0.93);
        --paper-strong: #fffaf4;
        --ink: #171b17;
        --muted: #5f5f57;
        --line: rgba(23, 27, 23, 0.12);
        --accent: #b34e2d;
        --accent-strong: #7d2d18;
        --sage: #40604b;
        --shadow: 0 22px 56px rgba(49, 31, 16, 0.13);
        --mono: "SFMono-Regular", "SF Mono", "JetBrains Mono", "Cascadia Code", Consolas, monospace;
      }}
      * {{ box-sizing: border-box; }}
      body {{
        margin: 0;
        min-height: 100vh;
        color: var(--ink);
        background:
          radial-gradient(circle at top left, rgba(179, 78, 45, 0.2), transparent 24%),
          radial-gradient(circle at bottom right, rgba(64, 96, 75, 0.2), transparent 28%),
          linear-gradient(180deg, #fbf7f1, var(--bg));
        font-family: "Iowan Old Style", "Palatino Linotype", "Book Antiqua", Palatino, Georgia, serif;
      }}
      .shell {{
        width: min(1220px, 100%);
        margin: 0 auto;
        padding: 2rem;
        display: grid;
        gap: 1rem;
      }}
      .hero, .panel {{
        background: var(--paper);
        border: 1px solid var(--line);
        border-radius: 30px;
        box-shadow: var(--shadow);
      }}
      .hero {{
        padding: clamp(1.5rem, 4vw, 3rem);
        overflow: hidden;
        position: relative;
      }}
      .hero::after {{
        content: "";
        position: absolute;
        inset: auto -8% -18% auto;
        width: 320px;
        height: 320px;
        border-radius: 50%;
        background: radial-gradient(circle, rgba(179, 78, 45, 0.16), transparent 68%);
        pointer-events: none;
      }}
      .eyebrow {{
        display: inline-flex;
        padding: 0.3rem 0.7rem;
        border-radius: 999px;
        border: 1px solid var(--line);
        background: rgba(255,255,255,0.82);
        font-family: var(--mono);
        font-size: 0.75rem;
        text-transform: uppercase;
        letter-spacing: 0.08em;
        color: var(--muted);
      }}
      h1 {{
        margin: 1rem 0 0.8rem;
        font-size: clamp(3rem, 8vw, 6.6rem);
        line-height: 0.9;
        letter-spacing: -0.06em;
      }}
      .lede {{
        max-width: 58rem;
        color: var(--muted);
        font-size: 1.14rem;
        line-height: 1.75;
      }}
      .hero-grid, .triptych, .docs-grid {{
        display: grid;
        gap: 1rem;
      }}
      .hero-grid {{
        grid-template-columns: 1.3fr 0.9fr;
        align-items: start;
        margin-top: 1.6rem;
      }}
      .triptych {{
        grid-template-columns: repeat(3, minmax(0, 1fr));
      }}
      .docs-grid {{
        grid-template-columns: repeat(4, minmax(0, 1fr));
      }}
      .card {{
        border: 1px solid var(--line);
        border-radius: 22px;
        background: var(--paper-strong);
        padding: 1rem 1.1rem;
      }}
      .card h2, .card h3 {{
        margin: 0 0 0.5rem;
        font-size: 1.2rem;
      }}
      p {{
        margin: 0;
        color: var(--muted);
        line-height: 1.7;
      }}
      pre {{
        margin: 0;
        padding: 1rem;
        overflow: auto;
        border-radius: 20px;
        background: #1d221e;
        color: #f8f4ec;
        font-family: var(--mono);
        font-size: 0.82rem;
        line-height: 1.6;
      }}
      .stats {{
        display: grid;
        grid-template-columns: repeat(4, minmax(0, 1fr));
        gap: 0.8rem;
        margin-top: 1.3rem;
      }}
      .stat {{
        padding: 0.9rem 1rem;
        border-radius: 18px;
        background: rgba(255,255,255,0.85);
        border: 1px solid var(--line);
      }}
      .stat-label {{
        display: block;
        margin-bottom: 0.45rem;
        color: var(--muted);
        font-family: var(--mono);
        font-size: 0.74rem;
        text-transform: uppercase;
        letter-spacing: 0.08em;
      }}
      .stat-value {{
        font-size: 1.8rem;
        line-height: 1;
      }}
      .section {{
        padding: 1.4rem;
      }}
      .section-title {{
        margin: 0 0 0.75rem;
        font-size: 1.7rem;
        line-height: 1.05;
      }}
      .link-grid {{
        display: grid;
        gap: 0.7rem;
      }}
      a {{
        color: var(--accent-strong);
        text-decoration-thickness: 2px;
        text-underline-offset: 0.18em;
      }}
      code {{
        font-family: var(--mono);
        font-size: 0.94em;
      }}
      ul {{
        margin: 0;
        padding-left: 1.15rem;
        color: var(--muted);
        line-height: 1.7;
      }}
      .muted {{
        color: var(--muted);
      }}
      @media (max-width: 1024px) {{
        .hero-grid, .triptych, .docs-grid, .stats {{
          grid-template-columns: 1fr;
        }}
      }}
    </style>
  </head>
  <body>
    <div class="shell">
      <section class="hero">
        <span class="eyebrow">Rust-first local automation plane</span>
        <h1>Tame hostile CLIs.</h1>
        <p class="lede">
          Switchboard gives humans, scripts, and LLMs one stable local contract across ugly real-world tools like
          GitHub and Google Workspace. You get explicit namespaces, isolated credentials, draft-first writes, approval
          gates, audit logs, undo metadata, and raw passthrough when the curated layer runs out.
        </p>
        <div class="stats">
          <div class="stat"><span class="stat-label">Total tools</span><span class="stat-value">{tool_count}</span></div>
          <div class="stat"><span class="stat-label">Curated</span><span class="stat-value">{curated_count}</span></div>
          <div class="stat"><span class="stat-label">GitHub</span><span class="stat-value">{github_tools}</span></div>
          <div class="stat"><span class="stat-label">Google</span><span class="stat-value">{google_tools}</span></div>
        </div>
        <div class="hero-grid">
          <div class="card">
            <h2>The problem</h2>
            <p>
              Existing automation stacks either leak provider-specific auth and command weirdness everywhere, or they
              hide it behind a cloud service that becomes the new trust boundary. Single-login-hostile CLIs like
              <code>gws</code> make it worse by stomping local state when you try to use multiple accounts.
            </p>
          </div>
          <div class="card">
            <h2>The move</h2>
            <p>
              Switchboard keeps the trust boundary local. Namespaces pin provider, account label, auth material, and
              optional <code>state_dir</code>, so tools that were never built for multi-login stop fighting each other.
            </p>
          </div>
        </div>
      </section>

      <section class="panel section">
        <h2 class="section-title">Install</h2>
        <div class="triptych">
          <div class="card">
            <h3>Source today</h3>
            <pre>cargo install --path crates/switchboard-cli</pre>
          </div>
          <div class="card">
            <h3>Nix today</h3>
            <pre>nix build .#switchboard</pre>
          </div>
          <div class="card">
            <h3>Release pipeline wired</h3>
            <p>
              Tagged releases are configured to publish checksummed archives plus shell and PowerShell installers.
              After the first public release, this is the main install path.
            </p>
          </div>
        </div>
      </section>

      <section class="panel section">
        <h2 class="section-title">Get started in five minutes</h2>
        <div class="triptych">
          <div class="card">
            <h3>1. Add namespaces</h3>
            <pre>switchboard ns list
switchboard tools list</pre>
          </div>
          <div class="card">
            <h3>2. Read first</h3>
            <pre>switchboard github.notifications.list \
  --ns github.personal \
  --json</pre>
          </div>
          <div class="card">
            <h3>3. Draft writes</h3>
            <pre>switchboard google.calendar.create \
  --ns google.work \
  --title "Vet visit" \
  --start "2026-04-01T09:00:00-07:00" \
  --end "2026-04-01T10:00:00-07:00" \
  --draft</pre>
          </div>
        </div>
      </section>

      <section class="panel section">
        <h2 class="section-title">Docs</h2>
        <div class="docs-grid">
          <div class="card">
            <h3>Reference explorer</h3>
            <p><a href="./reference/">Browse the live tool catalog</a></p>
          </div>
          <div class="card">
            <h3>README</h3>
            <p><a href="{repo_url}/blob/main/README.md">Problem, solution, quickstart</a></p>
          </div>
          <div class="card">
            <h3>LLM guide</h3>
            <p><a href="{repo_url}/blob/main/docs/llms/getting-started.md">Rules and operator patterns</a></p>
          </div>
          <div class="card">
            <h3>Roadmap</h3>
            <p><a href="{repo_url}/blob/main/docs/open-source-prime-time.md">Open source prime-time board</a></p>
          </div>
        </div>
      </section>

      <section class="panel section">
        <h2 class="section-title">Provider snapshot</h2>
        <div class="triptych">
          <div class="card">
            <h3>GitHub</h3>
            <p>{github_tools} tools, {github_curated} curated, {github_raw} raw. The curated layer focuses on reads, search, and comment primitives without giving up raw `gh` coverage.</p>
          </div>
          <div class="card">
            <h3>Google Workspace</h3>
            <p>{google_tools} tools, {google_curated} curated, {google_raw} raw. Namespace-scoped <code>state_dir</code> is what makes the multi-login story sane for CLIs that were not designed for it.</p>
          </div>
          <div class="card">
            <h3>OAuth helper</h3>
            <p><a href="./mychart-callback/">MyChart callback page</a> stays here because Epic still insists on HTTPS redirect URIs. Boring, but useful.</p>
          </div>
        </div>
      </section>
    </div>
  </body>
</html>
"#,
        tool_count = snapshot.stats.tool_count,
        curated_count = snapshot.stats.curated_count,
        github_tools = github.tool_count,
        github_curated = github.curated_count,
        github_raw = github.raw_count,
        google_tools = google.tool_count,
        google_curated = google.curated_count,
        google_raw = google.raw_count,
        repo_url = REPO_URL
    )
}

#[allow(clippy::uninlined_format_args)]
fn render_llms_txt_for_site() -> String {
    let site = SITE_URL;
    let repo = REPO_URL;
    format!(
        "\
# switchboard\n\
> Rust-first local automation plane for GitHub and Google Workspace with namespace-scoped auth, draft-first writes, approvals, audit, and raw CLI passthrough.\n\
\n\
{site} \n\
{site}reference/ \n\
{repo}/blob/main/README.md \n\
{repo}/blob/main/docs/llms/getting-started.md \n\
{repo}/blob/main/docs/llms/patterns.md \n\
{repo}/blob/main/docs/reference/README.md \n\
{repo}/blob/main/docs/reference/support-matrix.md \n\
{repo}/blob/main/docs/reference/catalog.json\n"
        ,
        site = site,
        repo = repo
    )
}

fn write_rendered_docs(rendered: &RenderedSite) -> Result<()> {
    let workspace_root = workspace_root();
    clean_generated_directory(&workspace_root.join("docs/reference"))?;

    for (path, contents) in &rendered.files {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
        }
        fs::write(path, contents).with_context(|| format!("failed to write {}", path.display()))?;
    }
    Ok(())
}

fn check_rendered_docs(rendered: &RenderedSite) -> Result<()> {
    for (path, expected) in &rendered.files {
        let actual = fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
        if normalize_newlines(&actual) != normalize_newlines(expected) {
            return Err(anyhow!(
                "generated file {} is stale, run `cargo run -p switchboard-cli --bin switchboard-docs-gen -- generate`",
                path.display()
            ));
        }
    }

    let workspace_root = workspace_root();
    check_generated_directory(
        &workspace_root.join("docs/reference"),
        rendered
            .files
            .keys()
            .filter(|path| path.starts_with(workspace_root.join("docs/reference")))
            .cloned()
            .collect(),
    )?;
    Ok(())
}

fn render_deploy_site(snapshot: &CatalogSnapshot, output_dir: &Path) -> Result<()> {
    clean_generated_directory(output_dir)?;
    let reference_dir = output_dir.join("reference");
    fs::create_dir_all(&reference_dir).with_context(|| format!("failed to create {}", reference_dir.display()))?;
    fs::write(output_dir.join("index.html"), render_site_index(snapshot))
        .with_context(|| format!("failed to write {}", output_dir.join("index.html").display()))?;
    fs::write(output_dir.join("llms.txt"), render_llms_txt_for_site())
        .with_context(|| format!("failed to write {}", output_dir.join("llms.txt").display()))?;
    fs::write(output_dir.join(".nojekyll"), "")
        .with_context(|| format!("failed to write {}", output_dir.join(".nojekyll").display()))?;
    fs::write(
        reference_dir.join("catalog.json"),
        format!(
            "{}\n",
            serde_json::to_string_pretty(snapshot).context("failed to serialize site catalog")?
        ),
    )
    .with_context(|| format!("failed to write {}", reference_dir.join("catalog.json").display()))?;
    fs::write(reference_dir.join("index.html"), render_reference_html(snapshot))
        .with_context(|| format!("failed to write {}", reference_dir.join("index.html").display()))?;

    let callback_source = workspace_root().join("pages/mychart-callback");
    let callback_target = output_dir.join("mychart-callback");
    copy_directory(&callback_source, &callback_target)?;
    Ok(())
}

fn check_generated_directory(root: &Path, expected: BTreeSet<PathBuf>) -> Result<()> {
    let mut actual = BTreeSet::new();
    if root.exists() {
        collect_files(root, &mut actual)?;
    }
    if actual != expected {
        return Err(anyhow!(
            "generated directory {} is stale, run `cargo run -p switchboard-cli --bin switchboard-docs-gen -- generate`",
            root.display()
        ));
    }
    Ok(())
}

fn collect_files(root: &Path, acc: &mut BTreeSet<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(root).with_context(|| format!("failed to read {}", root.display()))? {
        let entry = entry.context("failed to read directory entry")?;
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, acc)?;
        } else if path.is_file() {
            acc.insert(path);
        }
    }
    Ok(())
}

fn copy_directory(source: &Path, target: &Path) -> Result<()> {
    fs::create_dir_all(target).with_context(|| format!("failed to create {}", target.display()))?;
    for entry in fs::read_dir(source).with_context(|| format!("failed to read {}", source.display()))? {
        let entry = entry.context("failed to read directory entry")?;
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        if source_path.is_dir() {
            copy_directory(&source_path, &target_path)?;
        } else if source_path.is_file() {
            fs::copy(&source_path, &target_path)
                .with_context(|| format!("failed to copy {} to {}", source_path.display(), target_path.display()))?;
        }
    }
    Ok(())
}

fn clean_generated_directory(path: &Path) -> Result<()> {
    if path.exists() {
        fs::remove_dir_all(path).with_context(|| format!("failed to clear {}", path.display()))?;
    }
    fs::create_dir_all(path).with_context(|| format!("failed to create {}", path.display()))?;
    Ok(())
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn scratch_dir() -> PathBuf {
    std::env::temp_dir().join(format!(
        "switchboard-docs-gen-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos()
    ))
}

fn normalize_newlines(value: &str) -> String {
    value.replace("\r\n", "\n")
}

fn status_for(tool: &RegisteredTool) -> ToolCatalogStatus {
    if tool.surface == ToolSurface::Raw {
        ToolCatalogStatus::Raw
    } else if tool.execution_support == ToolExecutionSupport::PlanningOnly {
        ToolCatalogStatus::PlanningOnly
    } else {
        ToolCatalogStatus::Stable
    }
}

fn relative_to_reference_root(path: &str) -> &str {
    path.strip_prefix("docs/reference/").unwrap_or(path)
}
