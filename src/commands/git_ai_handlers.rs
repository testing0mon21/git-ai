use crate::authorship::range_authorship;
use crate::authorship::stats::stats_command;
use crate::authorship::working_log::{AgentId, CheckpointKind};
use crate::commands;
use crate::commands::checkpoint_agent::agent_presets::{
    AgentCheckpointFlags, AgentCheckpointPreset, AgentRunResult, ClaudePreset, CursorPreset,
    GithubCopilotPreset, RooodePreset,
};
use crate::commands::checkpoint_agent::agent_v1_preset::AgentV1Preset;
use crate::config;
use crate::git::find_repository;
use crate::git::find_repository_in_path;
use crate::git::repository::CommitRange;
use crate::utils::Timer;
use std::io::IsTerminal;
use std::io::Read;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn handle_git_ai(args: &[String]) {
    if args.is_empty() {
        print_help();
        return;
    }
    let timer = Timer::default();

    match args[0].as_str() {
        "help" | "--help" | "-h" => {
            print_help();
        }
        "version" | "--version" | "-v" => {
            println!(env!("CARGO_PKG_VERSION"));
            std::process::exit(0);
        }
        "stats-delta" => {
            handle_stats_delta(&args[1..]);
        }
        "stats" => {
            handle_stats(&args[1..]);
        }
        "checkpoint" => {
            let end = timer.start("git-ai checkpoint");
            handle_checkpoint(&args[1..]);
            end();
        }
        "blame" => {
            handle_ai_blame(&args[1..]);
        }
        "git-path" => {
            let config = config::Config::get();
            println!("{}", config.git_cmd());
            std::process::exit(0);
        }
        "install-hooks" => {
            if let Err(e) = commands::install_hooks::run(&args[1..]) {
                eprintln!("Install hooks failed: {}", e);
                std::process::exit(1);
            }
        }
        "squash-authorship" => {
            commands::squash_authorship::handle_squash_authorship(&args[1..]);
        }
        _ => {
            println!("Unknown git-ai command: {}", args[0]);
            std::process::exit(1);
        }
    }
}

fn print_help() {
    eprintln!("git-ai - git proxy with AI authorship tracking");
    eprintln!("");
    eprintln!("Usage: git-ai <command> [args...]");
    eprintln!("");
    eprintln!("Commands:");
    eprintln!("  checkpoint         Checkpoint working changes and attribute author");
    eprintln!("    Presets: claude, cursor, github-copilot, rooode, mock_ai");
    eprintln!(
        "    --hook-input <json|stdin>   JSON payload required by presets, or 'stdin' to read from stdin"
    );
    eprintln!("    --show-working-log          Display current working log");
    eprintln!("    --reset                     Reset working log");
    eprintln!("  blame <file>       Git blame with AI authorship overlay");
    eprintln!("  stats [commit]     Show AI authorship statistics for a commit");
    eprintln!("    --json                 Output in JSON format");
    eprintln!(
        "  stats-delta        Generate authorship logs for children of commits with working logs"
    );
    eprintln!("    --json                 Output created notes as JSON");
    eprintln!("  install-hooks      Install git hooks for AI authorship tracking");
    eprintln!("  squash-authorship  Generate authorship from squashed commits");
    eprintln!("    <branch> <new_sha> <old_sha>  Required: branch, new commit SHA, old commit SHA");
    eprintln!("    --dry-run             Show what would be done without making changes");
    eprintln!("  git-path           Print the path to the underlying git executable");
    eprintln!("  version, -v, --version     Print the git-ai version");
    eprintln!("  help, -h, --help           Show this help message");
    eprintln!("");
    std::process::exit(0);
}

fn handle_checkpoint(args: &[String]) {
    let mut repository_working_dir = std::env::current_dir()
        .unwrap()
        .to_string_lossy()
        .to_string();

    // Parse checkpoint-specific arguments
    let mut show_working_log = false;
    let mut reset = false;
    let mut hook_input = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--show-working-log" => {
                show_working_log = true;
                i += 1;
            }
            "--reset" => {
                reset = true;
                i += 1;
            }
            "--hook-input" => {
                if i + 1 < args.len() {
                    hook_input = Some(args[i + 1].clone());
                    if hook_input.as_ref().unwrap() == "stdin" {
                        let mut stdin = std::io::stdin();
                        let mut buffer = String::new();
                        if let Err(e) = stdin.read_to_string(&mut buffer) {
                            eprintln!("Failed to read stdin for hook input: {}", e);
                            std::process::exit(1);
                        }
                        if !buffer.trim().is_empty() {
                            hook_input = Some(buffer);
                        } else {
                            eprintln!("No hook input provided (via --hook-input or stdin).");
                            std::process::exit(1);
                        }
                    } else if hook_input.as_ref().unwrap().trim().is_empty() {
                        eprintln!("Error: --hook-input requires a value");
                        std::process::exit(1);
                    }
                    i += 2;
                } else {
                    eprintln!("Error: --hook-input requires a value or 'stdin' to read from stdin");
                    std::process::exit(1);
                }
            }

            _ => {
                i += 1;
            }
        }
    }

    let mut agent_run_result = None;
    // Handle preset arguments after parsing all flags
    if !args.is_empty() {
        match args[0].as_str() {
            "claude" => {
                match ClaudePreset.run(AgentCheckpointFlags {
                    hook_input: hook_input.clone(),
                }) {
                    Ok(agent_run) => {
                        if agent_run.repo_working_dir.is_some() {
                            repository_working_dir = agent_run.repo_working_dir.clone().unwrap();
                        }
                        agent_run_result = Some(agent_run);
                    }
                    Err(e) => {
                        eprintln!("Claude preset error: {}", e);
                        std::process::exit(1);
                    }
                }
            }
            "cursor" => {
                match CursorPreset.run(AgentCheckpointFlags {
                    hook_input: hook_input.clone(),
                }) {
                    Ok(agent_run) => {
                        if agent_run.repo_working_dir.is_some() {
                            repository_working_dir = agent_run.repo_working_dir.clone().unwrap();
                        }
                        agent_run_result = Some(agent_run);
                    }
                    Err(e) => {
                        eprintln!("Error running Cursor preset: {}", e);
                        std::process::exit(1);
                    }
                }
            }
            "github-copilot" => {
                match GithubCopilotPreset.run(AgentCheckpointFlags {
                    hook_input: hook_input.clone(),
                }) {
                    Ok(agent_run) => {
                        agent_run_result = Some(agent_run);
                    }
                    Err(e) => {
                        eprintln!("Github Copilot preset error: {}", e);
                        std::process::exit(1);
                    }
                }
            }
            "rooode" => {
                match RooodePreset.run(AgentCheckpointFlags {
                    hook_input: hook_input.clone(),
                }) {
                    Ok(agent_run) => {
                        if agent_run.repo_working_dir.is_some() {
                            repository_working_dir = agent_run.repo_working_dir.clone().unwrap();
                        }
                        agent_run_result = Some(agent_run);
                    }
                    Err(e) => {
                        eprintln!("Rooode preset error: {}", e);
                        std::process::exit(1);
                    }
                }
            }
            "agent-v1" => {
                match AgentV1Preset.run(AgentCheckpointFlags {
                    hook_input: hook_input.clone(),
                }) {
                    Ok(agent_run) => {
                        agent_run_result = Some(agent_run);
                    }
                    Err(e) => {
                        eprintln!("Agent V1 preset error: {}", e);
                        std::process::exit(1);
                    }
                }
            }
            "mock_ai" => {
                let mock_agent_id = format!(
                    "ai-thread-{}",
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map(|d| d.as_nanos())
                        .unwrap_or_else(|_| 0)
                );
                agent_run_result = Some(AgentRunResult {
                    agent_id: AgentId {
                        tool: "mock_ai".to_string(),
                        id: mock_agent_id,
                        model: "unknown".to_string(),
                    },
                    checkpoint_kind: CheckpointKind::AiAgent,
                    transcript: None,
                    repo_working_dir: None,
                    edited_filepaths: None,
                    will_edit_filepaths: None,
                });
            }
            _ => {}
        }
    }

    let final_working_dir = agent_run_result
        .as_ref()
        .and_then(|r| r.repo_working_dir.clone())
        .unwrap_or_else(|| repository_working_dir);
    // Find the git repository
    let repo = match find_repository_in_path(&final_working_dir) {
        Ok(repo) => repo,
        Err(e) => {
            eprintln!("Failed to find repository: {}", e);
            std::process::exit(1);
        }
    };

    let checkpoint_kind = agent_run_result
        .as_ref()
        .map(|r| r.checkpoint_kind)
        .unwrap_or(CheckpointKind::Human);

    // Get the current user name from git config
    let default_user_name = match repo.config_get_str("user.name") {
        Ok(Some(name)) if !name.trim().is_empty() => name,
        _ => {
            eprintln!("Warning: git user.name not configured. Using 'unknown' as author.");
            "unknown".to_string()
        }
    };

    if let Err(e) = commands::checkpoint::run(
        &repo,
        &default_user_name,
        checkpoint_kind,
        show_working_log,
        reset,
        false,
        agent_run_result,
    ) {
        eprintln!("Checkpoint failed: {}", e);
        std::process::exit(1);
    }
}

fn handle_stats_delta(args: &[String]) {
    // Parse stats-delta-specific arguments
    let mut json_output = false;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--json" => {
                json_output = true;
                i += 1;
            }
            _ => {
                eprintln!("Unknown stats-delta argument: {}", args[i]);
                std::process::exit(1);
            }
        }
    }

    // TODO: Do we have any 'global' args for the stats-delta?
    // Find the git repository
    let repo = match find_repository(&Vec::<String>::new()) {
        Ok(repo) => repo,
        Err(e) => {
            eprintln!("Failed to find repository: {}", e);
            std::process::exit(1);
        }
    };

    if let Err(e) = commands::stats_delta::run(&repo, json_output) {
        eprintln!("Stats delta failed: {}", e);
        std::process::exit(1);
    }
}

fn handle_ai_blame(args: &[String]) {
    if args.is_empty() {
        eprintln!("Error: blame requires a file argument");
        std::process::exit(1);
    }

    // TODO: Do we have any 'global' args for the ai-blame?
    // Find the git repository
    let repo = match find_repository(&Vec::<String>::new()) {
        Ok(repo) => repo,
        Err(e) => {
            eprintln!("Failed to find repository: {}", e);
            std::process::exit(1);
        }
    };

    // Parse blame arguments
    let (file_path, options) = match commands::blame::parse_blame_args(args) {
        Ok(result) => result,
        Err(e) => {
            eprintln!("Failed to parse blame arguments: {}", e);
            std::process::exit(1);
        }
    };

    // Check if this is an interactive terminal
    let is_interactive = std::io::stdout().is_terminal();

    if is_interactive && options.incremental {
        // For incremental mode in interactive terminal, we need special handling
        // This would typically involve a pager like less
        eprintln!("Error: incremental mode is not supported in interactive terminal");
        std::process::exit(1);
    }

    if let Err(e) = repo.blame(&file_path, &options) {
        eprintln!("Blame failed: {}", e);
        std::process::exit(1);
    }
}

fn handle_stats(args: &[String]) {
    // Find the git repository
    let repo = match find_repository(&Vec::<String>::new()) {
        Ok(repo) => repo,
        Err(e) => {
            eprintln!("Failed to find repository: {}", e);
            std::process::exit(1);
        }
    };
    // Parse stats-specific arguments
    let mut json_output = false;
    let mut commit_sha = None;
    let mut commit_range: Option<CommitRange> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--json" => {
                json_output = true;
                i += 1;
            }
            _ => {
                // First non-flag argument is treated as commit SHA or range
                if commit_sha.is_none() {
                    let arg = &args[i];
                    // Check if this is a commit range (contains "..")
                    if arg.contains("..") {
                        let parts: Vec<&str> = arg.split("..").collect();
                        if parts.len() == 2 {
                            match CommitRange::new_infer_refname(
                                &repo,
                                parts[0].to_string(),
                                parts[1].to_string(),
                                // @todo this is probably fine, but we might want to give users an option to override from this command.
                                None,
                            ) {
                                Ok(range) => {
                                    commit_range = Some(range);
                                }
                                Err(e) => {
                                    eprintln!("Failed to create commit range: {}", e);
                                    std::process::exit(1);
                                }
                            }
                        } else {
                            eprintln!("Invalid commit range format. Expected: <commit>..<commit>");
                            std::process::exit(1);
                        }
                    } else {
                        commit_sha = Some(arg.clone());
                    }
                    i += 1;
                } else {
                    eprintln!("Unknown stats argument: {}", args[i]);
                    std::process::exit(1);
                }
            }
        }
    }

    // Handle commit range if detected
    if let Some(range) = commit_range {
        match range_authorship::range_authorship(range, true) {
            Ok(stats) => {
                if json_output {
                    let json_str = serde_json::to_string(&stats).unwrap();
                    println!("{}", json_str);
                } else {
                    range_authorship::print_range_authorship_stats(&stats);
                }
            }
            Err(e) => {
                eprintln!("Range authorship failed: {}", e);
                std::process::exit(1);
            }
        }
        return;
    }

    if let Err(e) = stats_command(&repo, commit_sha.as_deref(), json_output) {
        match e {
            crate::error::GitAiError::Generic(msg) if msg.starts_with("No commit found:") => {
                eprintln!("{}", msg);
            }
            _ => {
                eprintln!("Stats failed: {}", e);
            }
        }
        std::process::exit(1);
    }
}
