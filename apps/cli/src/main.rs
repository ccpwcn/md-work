use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use anyhow::{Context as _, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};
use mdwork_core::archive::{self, ConflictPolicy, ImportMode, ImportOptions};
use mdwork_core::search::{SearchHit, SearchQuery, search_notebook};
use mdwork_core::{Notebook, Workspace};
use mdwork_export::exporter::{ExportEvent, ExportRequest};
use serde_json::json;
use uuid::Uuid;

#[derive(Parser)]
#[command(name = "md-work-cli", version, about = "墨笺 md-work command line interface")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// MCP server (stdio or Streamable HTTP)
    Mcp(McpArgs),
    /// Export a note to PDF or PNG
    Export(ExportArgs),
    /// Search notes in the workspace
    Search(SearchArgs),
    /// Notebook operations
    Notebook(NotebookArgs),
}

#[derive(Args)]
struct McpArgs {
    #[command(subcommand)]
    command: McpCommand,
}

#[derive(Subcommand)]
enum McpCommand {
    /// Serve MCP over stdio or Streamable HTTP
    Serve(ServeArgs),
}

#[derive(Args)]
struct ServeArgs {
    /// Use stdio transport
    #[arg(long, conflicts_with = "http")]
    stdio: bool,
    /// Use Streamable HTTP transport
    #[arg(long)]
    http: bool,
    /// HTTP port (Streamable HTTP only)
    #[arg(long, default_value_t = 11026)]
    port: u16,
}

#[derive(Args)]
struct ExportArgs {
    /// Path to a JSON export request (written by the GUI or other tools)
    #[arg(long)]
    request: PathBuf,
}

#[derive(Args)]
struct SearchArgs {
    /// Plain text or regular expression to find
    query: String,
    /// Notebook id or display name; omit to search every notebook
    #[arg(long)]
    notebook: Option<String>,
    /// Treat the query as a regular expression
    #[arg(long)]
    regex: bool,
    #[arg(long)]
    case_sensitive: bool,
    /// Maximum number of hits
    #[arg(long, default_value_t = 100)]
    limit: usize,
    /// Print JSON instead of `notebook:path:line: text`
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
struct NotebookArgs {
    #[command(subcommand)]
    command: NotebookCommand,
}

#[derive(Subcommand)]
enum NotebookCommand {
    /// List notebooks found in the workspace
    List {
        #[arg(long)]
        json: bool,
    },
    /// Export one notebook as a .zip archive
    Export(NotebookExportArgs),
    /// Import a notebook .zip archive into the workspace
    Import(NotebookImportArgs),
}

#[derive(Args)]
struct NotebookExportArgs {
    /// Notebook id or display name
    notebook: String,
    /// Output zip path; defaults to `<notebook folder>.zip` in the current directory
    #[arg(short, long)]
    output: Option<PathBuf>,
}

#[derive(Args)]
struct NotebookImportArgs {
    /// Notebook .zip archive produced by `notebook export`
    archive: PathBuf,
    /// How to treat a notebook with the same id/name already in the workspace
    #[arg(long, value_enum, default_value_t = ModeArg::Merge)]
    mode: ModeArg,
    /// What to do with conflicting files inside the notebook
    #[arg(long, value_enum, default_value_t = PolicyArg::KeepBoth)]
    policy: PolicyArg,
    /// Import as a new notebook with this display name
    #[arg(long)]
    rename_to: Option<String>,
    /// Print the import plan without changing anything
    #[arg(long)]
    dry_run: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum ModeArg {
    Merge,
    Overwrite,
    Rename,
}

impl From<ModeArg> for ImportMode {
    fn from(value: ModeArg) -> Self {
        match value {
            ModeArg::Merge => ImportMode::Merge,
            ModeArg::Overwrite => ImportMode::Overwrite,
            ModeArg::Rename => ImportMode::Rename,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum PolicyArg {
    KeepExisting,
    UseImported,
    KeepBoth,
}

impl From<PolicyArg> for ConflictPolicy {
    fn from(value: PolicyArg) -> Self {
        match value {
            PolicyArg::KeepExisting => ConflictPolicy::KeepExisting,
            PolicyArg::UseImported => ConflictPolicy::UseImported,
            PolicyArg::KeepBoth => ConflictPolicy::KeepBoth,
        }
    }
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Mcp(args) => match args.command {
            McpCommand::Serve(args) => {
                let runtime = tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()?;
                runtime.block_on(async {
                    if args.http {
                        mdwork_mcp::serve_http(args.port).await
                    } else {
                        mdwork_mcp::serve_stdio().await
                    }
                })
            }
        },
        Command::Export(args) => run_export(&args.request),
        Command::Search(args) => run_search(&args),
        Command::Notebook(args) => match args.command {
            NotebookCommand::List { json } => run_notebook_list(json),
            NotebookCommand::Export(args) => run_notebook_export(&args),
            NotebookCommand::Import(args) => run_notebook_import(&args),
        },
    }
}

fn run_export(path: &PathBuf) -> anyhow::Result<()> {
    let bytes = std::fs::read(path)?;
    let request: ExportRequest = serde_json::from_slice(&bytes)?;
    let mut stdout = std::io::stdout().lock();
    let result = mdwork_export::run(&request, &mut |event| {
        let _ = writeln!(stdout, "{}", serde_json::to_string(&event).unwrap_or_default());
        let _ = stdout.flush();
    });
    match result {
        Ok(_) => Ok(()),
        Err(err) => {
            let _ = writeln!(
                stdout,
                "{}",
                serde_json::to_string(&ExportEvent::Error {
                    message: err.to_string()
                })
                .unwrap_or_default()
            );
            let _ = stdout.flush();
            Err(err)
        }
    }
}

fn open_workspace() -> anyhow::Result<Workspace> {
    Ok(Workspace::open(&mdwork_mcp::resolve_workspace_root())?)
}

fn resolve_notebook(workspace: &Workspace, spec: &str) -> anyhow::Result<Notebook> {
    let notebooks = workspace.scan().notebooks;
    if let Ok(id) = Uuid::parse_str(spec.trim()) {
        if let Some(notebook) = notebooks.iter().find(|notebook| notebook.marker.id == id) {
            return Ok(notebook.clone());
        }
        bail!("找不到笔记本：{spec}");
    }
    let wanted = spec.trim().to_lowercase();
    let matches: Vec<&Notebook> = notebooks
        .iter()
        .filter(|notebook| {
            notebook.display_name().to_lowercase() == wanted
                || notebook
                    .root
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().to_lowercase() == wanted)
        })
        .collect();
    match matches.as_slice() {
        [notebook] => Ok((*notebook).clone()),
        [] => bail!("找不到笔记本：{spec}"),
        many => bail!(
            "笔记本名称不唯一：{spec}（{}）",
            many.iter()
                .map(|notebook| notebook.marker.id.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn run_search(args: &SearchArgs) -> anyhow::Result<()> {
    let workspace = open_workspace()?;
    let notebooks = match &args.notebook {
        Some(spec) => vec![resolve_notebook(&workspace, spec)?],
        None => workspace.scan().notebooks,
    };
    let mut query = SearchQuery::new(args.query.trim());
    if query.is_blank() {
        bail!("查询内容不能为空");
    }
    query.case_sensitive = args.case_sensitive;
    query.regex = args.regex;

    let cancel = AtomicBool::new(false);
    let mut hits: Vec<SearchHit> = Vec::new();
    let mut truncated = false;
    for notebook in &notebooks {
        if hits.len() >= args.limit {
            truncated = true;
            break;
        }
        let outcome = search_notebook(notebook, &query, &cancel, args.limit - hits.len())?;
        truncated |= outcome.truncated;
        hits.extend(outcome.hits);
    }
    hits.sort_by(|a, b| {
        (&a.notebook_name, &a.rel_path, a.line).cmp(&(&b.notebook_name, &b.rel_path, b.line))
    });

    if args.json {
        let value = json!(hits
            .iter()
            .map(|hit| json!({
                "notebook_id": hit.notebook_id.to_string(),
                "notebook_name": hit.notebook_name,
                "path": hit.rel_path,
                "line": hit.line,
                "column": hit.column,
                "text": hit.text,
            }))
            .collect::<Vec<_>>());
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        for hit in &hits {
            println!(
                "{}:{}:{}: {}",
                hit.notebook_name, hit.rel_path, hit.line, hit.text
            );
        }
        if truncated {
            eprintln!("结果已截断（--limit {}）", args.limit);
        }
    }
    Ok(())
}

fn run_notebook_list(json_output: bool) -> anyhow::Result<()> {
    let workspace = open_workspace()?;
    let scan = workspace.scan();
    for issue in &scan.issues {
        eprintln!("警告：{}：{}", issue.path.display(), issue.message);
    }
    if json_output {
        let value = json!(scan
            .notebooks
            .iter()
            .map(|notebook| json!({
                "id": notebook.marker.id.to_string(),
                "name": notebook.display_name(),
                "path": notebook.root.display().to_string(),
                "notes": notebook.list_entries().map(|entries| entries.len()).unwrap_or(0),
            }))
            .collect::<Vec<_>>());
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        for notebook in &scan.notebooks {
            let notes = notebook
                .list_entries()
                .map(|entries| entries.len())
                .unwrap_or(0);
            println!(
                "{}  {}  {} 篇  {}",
                notebook.marker.id,
                notebook.display_name(),
                notes,
                notebook.root.display()
            );
        }
    }
    Ok(())
}

fn run_notebook_export(args: &NotebookExportArgs) -> anyhow::Result<()> {
    let workspace = open_workspace()?;
    let notebook = resolve_notebook(&workspace, &args.notebook)?;
    let output = args.output.clone().unwrap_or_else(|| {
        let folder = notebook
            .root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "notebook".to_string());
        Path::new(&folder).with_extension("zip")
    });
    let summary = archive::export_zip(&notebook, &output)?;
    println!(
        "已导出 {} 个文件（{} 字节）到 {}",
        summary.files,
        summary.bytes,
        output.display()
    );
    Ok(())
}

fn run_notebook_import(args: &NotebookImportArgs) -> anyhow::Result<()> {
    let workspace = open_workspace()?;
    let mode = ImportMode::from(args.mode);
    let plan = archive::plan_import(
        workspace.root(),
        &args.archive,
        mode,
        args.rename_to.as_deref(),
    )?;
    println!(
        "目标：{}（{}）新文件 {}，冲突 {}，相同 {}，共 {} 字节",
        plan.folder_name,
        if plan.target_exists { "已存在" } else { "新建" },
        plan.new_files,
        plan.conflicts.len(),
        plan.identical_files,
        plan.total_bytes
    );
    for conflict in &plan.conflicts {
        println!("  冲突：{}", conflict.path);
    }
    if args.dry_run {
        println!("（--dry-run：未写入）");
        return Ok(());
    }

    let options = ImportOptions {
        mode,
        policy: ConflictPolicy::from(args.policy),
        choices: Default::default(),
        rename_to: args.rename_to.clone(),
    };
    let outcome = archive::import_zip(workspace.root(), &args.archive, &options)
        .with_context(|| format!("导入 {}", args.archive.display()))?;
    workspace.refresh()?;
    println!(
        "已导入「{}」：写入 {}，保留旧版 {}，保留两份 {}，跳过相同 {}",
        outcome.notebook.display_name(),
        outcome.imported,
        outcome.kept_existing,
        outcome.kept_both,
        outcome.skipped_identical
    );
    if let Some(replaced) = outcome.replaced {
        println!("旧版本已备份：{}", replaced.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_mode_and_policy_map_to_core() {
        assert_eq!(ImportMode::from(ModeArg::Merge), ImportMode::Merge);
        assert_eq!(ImportMode::from(ModeArg::Overwrite), ImportMode::Overwrite);
        assert_eq!(ImportMode::from(ModeArg::Rename), ImportMode::Rename);
        assert_eq!(
            ConflictPolicy::from(PolicyArg::KeepBoth),
            ConflictPolicy::KeepBoth
        );
    }
}
