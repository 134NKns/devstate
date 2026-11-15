use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use clap::{CommandFactory, Parser, Subcommand};
use serde_json::{Value, json};

use devstate::queries::TaskCursor;
use devstate::registry::Registry;
use devstate::store::{Result, Store};
use devstate::mutations::{NewTask, Checkpoint, CloseTask, Verification};

mod output;
mod mcp;
mod payload;

#[derive(Parser)]
#[command(version, about = "Pick up where you left off.",
    after_help = "Start: devstate init --name my-project\nContinue: devstate resume\nAcross projects: devstate --registry PATH overview\nFor agents: add --json")]
struct Cli {
    /// Exact project root; defaults to the current directory, never a global last project.
    #[arg(long, global = true, default_value = ".")]
    root: PathBuf,
    /// Explicit registry file, required by register/unregister/overview.
    #[arg(long, global = true)]
    registry: Option<PathBuf>,
    /// Compact machine-readable JSON instead of the default human view.
    #[arg(long, global = true)]
    json: bool,
    /// Indented JSON; implies JSON output.
    #[arg(long, global = true)]
    pretty: bool,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Serve MCP JSON-RPC over stdio. Tools require explicit absolute roots.
    Mcp,
    /// Retrieve only relevant devstate usage guidance; no project DB required.
    Guide {
        #[arg(default_value = "start")]
        query: String,
        #[arg(long, default_value_t = 2, value_parser = clap::value_parser!(u32).range(1..=5))]
        limit: u32,
    },
    /// Initialize one project DB. Never overwrite an existing file.
    Init {
        #[arg(long)]
        name: String,
    },
    /// Resume focused work, or discover recent tasks when no focus is selected.
    Resume {
        /// Explicitly recompute verification freshness from source.
        #[arg(long)]
        check: bool,
    },
    /// Brief task context; request notes/evidence/full only when needed.
    Task {
        id: String,
        #[arg(long, default_value = "brief", value_parser = ["brief", "notes", "evidence", "usage", "full"])]
        section: String,
        #[arg(long)]
        check: bool,
    },
    /// Create a todo task without changing current focus.
    Create {
        #[arg(long)] title: String,
        #[arg(long, default_value = "task", value_parser = ["task", "bug"])] kind: String,
        #[arg(long)] spec: Option<String>,
    },
    /// Select a task; requires the project revision returned by resume.
    Focus {
        /// Omit ID to clear focus.
        id: Option<String>,
        #[arg(long)] revision: i64,
    },
    /// Persist current progress. Verification is an optional JSON object.
    Checkpoint {
        id: String,
        #[arg(long)] revision: i64,
        #[arg(long)] summary: String,
        #[arg(long)] next: String,
        #[arg(long, default_value = "active", value_parser = ["todo", "active", "blocked"])] status: String,
        #[arg(long, default_value = "implement")] phase: String,
        #[arg(long, default_value = "")] blockers: String,
        #[arg(long)] notes: Option<String>,
        #[arg(long)] verification: Option<String>,
    },
    /// Finish/cancel a task; clears focus atomically if this task was selected.
    Close {
        id: String,
        #[arg(long)] revision: i64,
        #[arg(long)] project_revision: i64,
        #[arg(long)] summary: String,
        #[arg(long, default_value = "done", value_parser = ["done", "cancelled"])] status: String,
        /// Explicit reason that verification is not required for this completion.
        #[arg(long)] reason: Option<String>,
    },
    /// List tasks with optional filters and a JSON keyset cursor.
    Tasks {
        #[arg(long, value_parser = ["todo", "active", "blocked", "done", "cancelled"])]
        status: Option<String>,
        #[arg(long, value_parser = ["task", "bug"])]
        kind: Option<String>,
        #[arg(long, default_value_t = 5, value_parser = clap::value_parser!(u32).range(1..=50))]
        limit: u32,
        /// Copy the next_cursor object from the previous page as a JSON string.
        #[arg(long)]
        cursor: Option<String>,
    },
    /// List recently updated work across all task statuses.
    Recent {
        #[arg(long, default_value_t = 5, value_parser = clap::value_parser!(u32).range(1..=50))]
        limit: u32,
    },
    /// Literal substring search of task titles, summaries and notes.
    Search {
        text: String,
        #[arg(long, default_value_t = 5, value_parser = clap::value_parser!(u32).range(1..=50))]
        limit: u32,
    },
    /// Capture source identity without running checks or recording evidence.
    Fingerprint,
    /// Register --root in --registry; creates a registry when needed.
    Register,
    /// Remove a saved path from --registry, even if the directory is missing.
    Unregister { saved_root: String },
    /// Explicit summary across registered projects; never changes local focus.
    Overview {
        #[arg(long, default_value_t = 5, value_parser = clap::value_parser!(u32).range(1..=50))]
        limit: u32,
        /// Root-path next_cursor from the previous overview page.
        #[arg(long)]
        cursor: Option<String>,
    },
}

fn main() -> ExitCode {
    if std::env::args_os().len() == 1 {
        let result = Cli::command().print_help();
        println!();
        return if result.is_ok() { ExitCode::SUCCESS } else { ExitCode::FAILURE };
    }
    let cli = Cli::parse();
    if matches!(cli.command, Commands::Mcp) {
        return match mcp::serve(io::stdin().lock(), io::stdout().lock()) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("devstate MCP: {}", output::safe(&error.to_string()));
                ExitCode::FAILURE
            }
        };
    }
    match run(&cli).and_then(|value| {
        if cli.json || cli.pretty {
            write_json(&value, cli.pretty)
        } else {
            let styled = io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none()
                && std::env::var("TERM").as_deref() != Ok("dumb");
            let text = output::render(&cli.command, &value, styled);
            io::stdout().lock().write_all(text.as_bytes())?;
            Ok(())
        }
    }) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            // Runtime errors go only to stderr; stdout remains machine-readable.
            if cli.json || cli.pretty {
                eprintln!("{}", json!({"error": error.to_string()}));
            } else {
                eprintln!("devstate: {}", output::safe(&error.to_string()));
            }
            ExitCode::FAILURE
        }
    }
}

fn run(cli: &Cli) -> Result<Value> {
    match &cli.command {
        Commands::Mcp => Err("MCP must run through the stdio server".into()),
        Commands::Guide { query, limit } => Ok(serde_json::to_value(devstate::guide::retrieve(query, *limit)?)?),
        Commands::Create { title, kind, spec } => {
            let result = Store::open(&cli.root)?.create_task(NewTask { title: title.clone(), kind: kind.clone(), spec_path: spec.clone() })?;
            Ok(serde_json::to_value(result)?)
        }
        Commands::Focus { id, revision } => {
            Ok(serde_json::to_value(Store::open(&cli.root)?.set_focus(id.as_deref(), *revision)?)?)
        }
        Commands::Checkpoint { id, revision, summary, next, status, phase, blockers, notes, verification } => {
            let verification = verification.as_deref().map(serde_json::from_str::<Verification>).transpose()?;
            let result = Store::open(&cli.root)?.checkpoint(Checkpoint {
                id: id.clone(), expected_revision: *revision, summary: summary.clone(), next_action: next.clone(),
                status: status.clone(), phase: phase.clone(), blockers: blockers.clone(), notes: notes.clone(), verification,
            })?;
            Ok(serde_json::to_value(result)?)
        }
        Commands::Close { id, revision, project_revision, summary, status, reason } => {
            let result = Store::open(&cli.root)?.close_task(CloseTask {
                id: id.clone(), expected_revision: *revision, expected_project_revision: *project_revision,
                summary: summary.clone(), status: status.clone(), not_required_reason: reason.clone(),
            })?;
            Ok(serde_json::to_value(result)?)
        }
        Commands::Init { name } => {
            let store = Store::init(&cli.root, name)?;
            Ok(serde_json::to_value(store.project()?)?)
        }
        Commands::Register => {
            let path = registry_path(cli, false)?;
            // Validate project before creating a registry for a mistyped root.
            let project = Store::open_read_only(&cli.root)?.project()?;
            Registry::open(path)?.register(&cli.root)?;
            Ok(json!({"registered": true, "project": project}))
        }
        Commands::Unregister { saved_root } => {
            let removed = Registry::open(registry_path(cli, true)?)?.unregister(saved_root)?;
            Ok(json!({"removed": removed, "root_path": saved_root}))
        }
        Commands::Overview { limit, cursor } => {
            let page = Registry::open_read_only(registry_path(cli, true)?)?.overview(*limit, cursor.as_deref())?;
            Ok(payload::overview(serde_json::to_value(page)?))
        }
        command => {
            let store = Store::open_read_only(&cli.root)?;
            match command {
                Commands::Resume { check } => {
                    let mut resume = if *check { store.resume_checked()? } else { store.resume()? };
                    let mut truncated_fields = Vec::new();
                    if let Some(task) = &mut resume.task {
                        for (name, text, limit) in [
                            ("summary", &mut task.summary, 240),
                            ("next_action", &mut task.next_action, 200),
                            ("blockers", &mut task.blockers, 200),
                        ] {
                            if text.chars().count() > limit {
                                *text = text.chars().take(limit).collect();
                                truncated_fields.push(name);
                            }
                        }
                    }
                    let mut value = json!({"project": {"name":resume.project.name,"root_path":resume.project.root_path,"revision":resume.project.revision}, "task": resume.task,
                        "truncated_fields": truncated_fields});
                    if let Some(fallback) = resume.fallback {
                        value["fallback"] = serde_json::to_value(fallback)?;
                    }
                    Ok(value)
                }
                Commands::Task { id, section, check } => {
                    let task = if *check { store.get_task_checked(id)? } else { store.get_task(id)? }.ok_or("task not found in this project")?;
                    let now = i64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis())?;
                    let elapsed = task.elapsed_ms(now);
                    let total = task.input_tokens.zip(task.output_tokens)
                        .and_then(|(input, output)| input.checked_add(output));
                    Ok(payload::task(json!({"task": task, "elapsed_ms": elapsed, "total_tokens": total}), section))
                }
                Commands::Tasks { status, kind, limit, cursor } => {
                    let cursor: Option<TaskCursor> = cursor.as_deref().map(|text| -> Result<TaskCursor> {
                        if text.len() > 8192 {
                            return Err("cursor exceeds 8192 bytes".into());
                        }
                        Ok(serde_json::from_str::<TaskCursor>(text)?)
                    }).transpose()?;
                    Ok(serde_json::to_value(store.list_tasks_filtered(
                        status.as_deref(), kind.as_deref(), *limit, cursor.as_ref(),
                    )?)?)
                }
                Commands::Recent { limit } => Ok(serde_json::to_value(store.recent_tasks(*limit)?)?),
                Commands::Search { text, limit } => Ok(serde_json::to_value(store.search_tasks(text, *limit)?)?),
                Commands::Fingerprint => Ok(serde_json::to_value(store.capture_source()?)?),
                _ => unreachable!("registry/init commands are handled above"),
            }
        }
    }
}

fn registry_path(cli: &Cli, must_exist: bool) -> Result<&Path> {
    let path = cli.registry.as_deref().ok_or("supply --registry PATH explicitly")?;
    if must_exist && !path.is_file() {
        return Err("registry does not exist; register a project first".into());
    }
    Ok(path)
}

fn write_json(value: &Value, pretty: bool) -> Result<()> {
    let mut stdout = io::stdout().lock();
    if pretty {
        serde_json::to_writer_pretty(&mut stdout, value)?;
    } else {
        serde_json::to_writer(&mut stdout, value)?;
    }
    writeln!(stdout)?;
    Ok(())
}
