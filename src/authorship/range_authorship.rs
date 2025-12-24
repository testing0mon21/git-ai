use std::collections::HashMap;
use std::collections::HashSet;

use serde::Deserialize;
use serde::Serialize;

use crate::authorship::stats::CommitStats;
use crate::error::GitAiError;
use crate::git::refs::{CommitAuthorship, get_commits_with_notes_from_list};
use crate::git::repository::{CommitRange, Repository};
use crate::utils::debug_log;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RangeAuthorshipStats {
    pub authorship_stats: AuthorshipStats,
    pub range_stats: CommitStats,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorshipStats {
    pub total_commits: usize,
    pub commits_with_authorship: usize,
    pub authors_commiting_authorship: HashSet<String>,
    pub authors_not_commiting_authorship: HashSet<String>,
    pub commits_without_authorship: Vec<String>,
    pub commits_without_authorship_with_authors: Vec<(String, String)>, // (sha, git_author)
}

pub fn range_authorship(
    commit_range: CommitRange,
    pre_fetch_contents: bool,
) -> Result<RangeAuthorshipStats, GitAiError> {
    if let Err(e) = commit_range.is_valid() {
        return Err(e);
    }

    // Fetch the branch if pre_fetch_contents is true
    if pre_fetch_contents {
        let repository = commit_range.repo();
        let refname = &commit_range.refname;

        // Get default remote, fallback to "origin" if not found
        let default_remote = repository
            .get_default_remote()?
            .unwrap_or_else(|| "origin".to_string());

        // Extract remote and branch from refname
        let (remote, fetch_refspec) = if refname.starts_with("refs/remotes/") {
            // Remote branch: refs/remotes/origin/branch-name -> origin, refs/heads/branch-name
            let without_prefix = refname.strip_prefix("refs/remotes/").unwrap();
            let parts: Vec<&str> = without_prefix.splitn(2, '/').collect();
            if parts.len() == 2 {
                (parts[0].to_string(), format!("refs/heads/{}", parts[1]))
            } else {
                (default_remote.clone(), refname.to_string())
            }
        } else if refname.starts_with("refs/heads/") {
            // Local branch: refs/heads/branch-name -> default_remote, refs/heads/branch-name
            (default_remote.clone(), refname.to_string())
        } else if refname.contains('/') && !refname.starts_with("refs/") {
            // Simple remote format: origin/branch-name -> origin, refs/heads/branch-name
            let parts: Vec<&str> = refname.splitn(2, '/').collect();
            if parts.len() == 2 {
                (parts[0].to_string(), format!("refs/heads/{}", parts[1]))
            } else {
                (default_remote.clone(), format!("refs/heads/{}", refname))
            }
        } else {
            // Plain branch name: branch-name -> default_remote, refs/heads/branch-name
            (default_remote.clone(), format!("refs/heads/{}", refname))
        };

        let mut args = repository.global_args_for_exec();
        args.push("fetch".to_string());
        args.push(remote.clone());
        args.push(fetch_refspec.clone());

        let output = crate::git::repository::exec_git(&args)?;

        if !output.status.success() {
            return Err(GitAiError::Generic(format!(
                "Failed to fetch {} from {}: {}",
                fetch_refspec,
                remote,
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        debug_log(&format!("✓ Fetched {} from {}", fetch_refspec, remote));
    }

    // First, collect all commit SHAs from the range
    let repository = commit_range.repo();
    let _refname = commit_range.refname.clone();

    // Extract start/end before consuming commit_range
    let start_sha = commit_range.start_oid.clone();
    let end_sha = commit_range.end_oid.clone();

    let commit_shas: Vec<String> = commit_range
        .into_iter()
        .map(|c| c.id().to_string())
        .collect();
    let commit_authorship = get_commits_with_notes_from_list(repository, &commit_shas)?;

    // Calculate range stats using the extracted start/end and pre-loaded commit_authorship
    let range_stats =
        calculate_range_stats_direct(repository, &start_sha, &end_sha, &commit_authorship)?;

    Ok(RangeAuthorshipStats {
        authorship_stats: AuthorshipStats {
            total_commits: commit_authorship.len(),
            commits_with_authorship: commit_authorship
                .iter()
                .filter(|ca| matches!(ca, CommitAuthorship::Log { .. }))
                .count(),
            authors_commiting_authorship: commit_authorship
                .iter()
                .filter_map(|ca| match ca {
                    CommitAuthorship::Log { git_author, .. } => Some(git_author.clone()),
                    _ => None,
                })
                .collect(),
            authors_not_commiting_authorship: commit_authorship
                .iter()
                .filter_map(|ca| match ca {
                    CommitAuthorship::NoLog { git_author, .. } => Some(git_author.clone()),
                    _ => None,
                })
                .collect(),
            commits_without_authorship: commit_authorship
                .iter()
                .filter_map(|ca| match ca {
                    CommitAuthorship::NoLog { sha, .. } => Some(sha.clone()),
                    _ => None,
                })
                .collect(),
            commits_without_authorship_with_authors: commit_authorship
                .iter()
                .filter_map(|ca| match ca {
                    CommitAuthorship::NoLog { sha, git_author } => {
                        Some((sha.clone(), git_author.clone()))
                    }
                    _ => None,
                })
                .collect(),
        },
        range_stats,
    })
}

/// Calculate AI vs human line contributions for a commit range
/// by diffing start->end and using blame to determine authorship
fn calculate_range_stats_direct(
    repo: &Repository,
    start_sha: &str,
    end_sha: &str,
    commit_authorship: &[CommitAuthorship],
) -> Result<CommitStats, GitAiError> {
    // Cache for foreign prompts to avoid repeated grepping
    let mut foreign_prompts_cache: HashMap<
        String,
        Option<crate::authorship::authorship_log::PromptRecord>,
    > = HashMap::new();
    // Get the diff using git diff to ensure consistency with git's view
    let mut args = repo.global_args_for_exec();
    args.push("diff".to_string());
    args.push(format!("{}..{}", start_sha, end_sha));

    let output = crate::git::repository::exec_git(&args)?;
    let diff_output = String::from_utf8(output.stdout)?;

    // Parse diff to extract added lines and their line numbers
    let added_lines_by_file = parse_git_diff_for_added_lines(&diff_output)?;

    let mut git_diff_added_lines = 0u32;
    let mut git_diff_deleted_lines = 0u32;

    // First pass: count total additions/deletions from the diff
    for line in diff_output.lines() {
        if line.starts_with('+') && !line.starts_with("+++") {
            git_diff_added_lines += 1;
        } else if line.starts_with('-') && !line.starts_with("---") {
            git_diff_deleted_lines += 1;
        }
    }

    // Build authorship logs HashMap from pre-loaded commit_authorship
    let mut auth_logs: HashMap<
        String,
        Option<crate::authorship::authorship_log_serialization::AuthorshipLog>,
    > = HashMap::new();
    for commit in commit_authorship {
        match commit {
            CommitAuthorship::Log {
                sha,
                authorship_log,
                ..
            } => {
                auth_logs.insert(sha.clone(), Some(authorship_log.clone()));
            }
            CommitAuthorship::NoLog { sha, .. } => {
                auth_logs.insert(sha.clone(), None);
            }
        }
    }

    // Build blame cache for each file
    let mut blame_cache: HashMap<String, FileBlame> = HashMap::new();
    for file_path in added_lines_by_file.keys() {
        let file_blame = compute_file_blame(
            repo,
            file_path,
            end_sha,
            &auth_logs,
            &mut foreign_prompts_cache,
        )?;

        blame_cache.insert(file_path.clone(), file_blame);
    }

    // Second pass: for each added line, lookup authorship from cache
    let mut human_additions = 0u32;
    let mut ai_additions = 0u32;
    let mut ai_accepted = 0u32;

    for (file_path, line_numbers) in added_lines_by_file {
        if let Some(file_blame) = blame_cache.get(&file_path) {
            for line_no in line_numbers {
                if let Some((_, is_ai)) = file_blame.get_line_authorship(line_no) {
                    if is_ai {
                        ai_additions += 1;
                        ai_accepted += 1;
                    } else {
                        human_additions += 1;
                    }
                } else {
                    // Could not determine, count as human
                    human_additions += 1;
                }
            }
        }
    }
    Ok(CommitStats {
        human_additions,
        mixed_additions: 0,
        ai_additions,
        ai_accepted,
        time_waiting_for_ai: 0,
        ai_input_tokens: 0,
        ai_output_tokens: 0,
        ai_cache_read_input_tokens: 0,
        ai_cache_creation_input_tokens: 0,
        git_diff_deleted_lines,
        git_diff_added_lines,
    })
}

/// Cache of blame information for a single file
struct FileBlame {
    // Map from line_number to (commit_sha, is_ai_authored)
    line_blame: HashMap<u32, (String, bool)>,
}

impl FileBlame {
    /// Get authorship info for a specific line
    /// Returns Some((commit_sha, is_ai_authored)) or None if line not found
    fn get_line_authorship(&self, line_no: u32) -> Option<(String, bool)> {
        self.line_blame.get(&line_no).cloned()
    }
}

/// Compute blame for an entire file and extract authorship info
fn compute_file_blame(
    repo: &Repository,
    file_path: &str,
    context_commit_sha: &str,
    auth_logs: &HashMap<
        String,
        Option<crate::authorship::authorship_log_serialization::AuthorshipLog>,
    >,
    foreign_prompts_cache: &mut HashMap<
        String,
        Option<crate::authorship::authorship_log::PromptRecord>,
    >,
) -> Result<FileBlame, GitAiError> {
    use crate::commands::blame::GitAiBlameOptions;

    let mut blame_opts = GitAiBlameOptions::default();
    blame_opts.newest_commit = Some(context_commit_sha.to_string());

    // Get blame hunks for the entire file
    let blame_hunks = repo.blame_hunks(file_path, 1, u32::MAX, &blame_opts)?;

    let mut line_blame: HashMap<u32, (String, bool)> = HashMap::new();

    // Process each blame hunk
    for hunk in blame_hunks {
        let commit_sha = hunk.commit_sha.clone();

        // Look up the AI authorship log for this commit
        let is_ai = match auth_logs.get(&commit_sha) {
            Some(Some(authorship_log)) => {
                // Check if any lines in this hunk are AI-authored
                let orig_line_start = hunk.orig_range.0;
                let orig_line_end = hunk.orig_range.1;

                // Check if at least one line in the hunk is AI-authored
                (orig_line_start..=orig_line_end).any(|line_no| {
                    authorship_log
                        .get_line_attribution(repo, file_path, line_no, foreign_prompts_cache)
                        .is_some_and(|(_, _, prompt)| prompt.is_some())
                })
            }
            _ => false, // No authorship log means human-authored
        };

        // Record authorship for each line in the hunk
        let new_line_start = hunk.range.0;
        let new_line_end = hunk.range.1;
        for new_line_no in new_line_start..=new_line_end {
            line_blame.insert(new_line_no, (commit_sha.clone(), is_ai));
        }
    }

    Ok(FileBlame { line_blame })
}

/// Parse git diff output to extract which lines were added in each file
/// Returns a map of file_path -> Vec<line_numbers> where line numbers are 1-indexed
fn parse_git_diff_for_added_lines(
    diff_output: &str,
) -> Result<HashMap<String, Vec<u32>>, GitAiError> {
    let mut result: HashMap<String, Vec<u32>> = HashMap::new();
    let mut current_file: Option<String> = None;
    let mut new_line_num: u32 = 0;

    for line in diff_output.lines() {
        // Parse file headers: +++ b/path/to/file
        if line.starts_with("+++") {
            let file_path = line
                .strip_prefix("+++")
                .unwrap_or("")
                .trim()
                .strip_prefix("b/")
                .unwrap_or("")
                .to_string();
            current_file = Some(file_path);
            new_line_num = 0;
            continue;
        }

        // Parse hunk headers: @@ -old_start,old_count +new_start,new_count @@
        if line.starts_with("@@") {
            if let Some(end_idx) = line.rfind("@@") {
                let hunk_header = &line[2..end_idx];
                // Parse the +new_start,new_count part
                if let Some(plus_idx) = hunk_header.find('+') {
                    let new_part = &hunk_header[plus_idx + 1..];
                    let parts: Vec<&str> = new_part.split(',').collect();
                    if let Ok(start) = parts[0].parse::<u32>() {
                        new_line_num = start;
                    }
                }
            }
            continue;
        }

        // Skip diff metadata lines
        if line.starts_with("diff --git") || line.starts_with("index ") || line.starts_with("---") {
            continue;
        }

        // Process content lines
        if let Some(ref file) = current_file {
            if line.starts_with('+') && !line.starts_with("+++") {
                // Added line - record its line number
                result
                    .entry(file.clone())
                    .or_insert_with(Vec::new)
                    .push(new_line_num);
                new_line_num += 1;
            } else if line.starts_with('-') && !line.starts_with("---") {
                // Deleted line - don't advance new_line_num
            } else if !line.starts_with('\\') {
                // Regular context line or empty line
                new_line_num += 1;
            }
        }
    }

    Ok(result)
}

pub fn print_range_authorship_stats(stats: &RangeAuthorshipStats) {
    println!("\n");
    // Check if any commits have authorship logs
    let has_any_authorship = stats.authorship_stats.commits_with_authorship > 0;
    let all_have_authorship =
        stats.authorship_stats.commits_with_authorship == stats.authorship_stats.total_commits;

    // If none of the commits have authorship logs, show the special message
    if !has_any_authorship {
        println!("Committers are not using git-ai");
        return;
    }

    // Use existing stats terminal output
    use crate::authorship::stats::write_stats_to_terminal;
    write_stats_to_terminal(&stats.range_stats, true);

    // If not all commits have authorship logs, show the breakdown
    if !all_have_authorship {
        let commits_without =
            stats.authorship_stats.total_commits - stats.authorship_stats.commits_with_authorship;
        let commit_word = if commits_without == 1 {
            "commit"
        } else {
            "commits"
        };
        println!(
            "  {} {} without Authorship Logs",
            commits_without, commit_word
        );

        // Show each commit without authorship
        for (sha, author) in &stats
            .authorship_stats
            .commits_without_authorship_with_authors
        {
            println!("    {} {}", &sha[0..7], author);
        }
    }
}
