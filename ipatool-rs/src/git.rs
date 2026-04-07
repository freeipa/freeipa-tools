use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::process::{Command, Stdio};

#[derive(Debug)]
pub struct ProcessResult {
    pub stdout: String,
    pub stderr: String,
    pub returncode: i32,
}

pub fn run_process(
    argv: &[&str],
    env: &HashMap<String, String>,
    stdin_data: Option<&str>,
    check_returncode: bool,
    timeout_secs: Option<u64>,
    verbosity: u8,
) -> Result<ProcessResult> {
    if verbosity > 0 {
        let cmd_repr = argv
            .iter()
            .map(|a| shell_quote(a))
            .collect::<Vec<_>>()
            .join(" ");
        eprintln!("\x1b[34m{}\x1b[0m", cmd_repr);
    }
    if verbosity > 2 {
        if let Some(data) = stdin_data {
            eprintln!("\x1b[33m{}\x1b[0m", data.trim_end());
        }
    }

    let mut cmd = Command::new(argv[0]);
    cmd.args(&argv[1..]);
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    // Merge current env with provided overrides
    for (k, v) in env {
        cmd.env(k, v);
    }

    let mut child = cmd.spawn().with_context(|| format!("Failed to spawn: {}", argv[0]))?;

    if let Some(data) = stdin_data {
        use std::io::Write;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(data.as_bytes())
                .with_context(|| format!("Writing stdin to: {}", argv[0]))?;
        }
    }

    let output = child
        .wait_with_output()
        .with_context(|| format!("Failed to wait for: {}", argv[0]))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let returncode = output.status.code().unwrap_or(-1);

    let failed = check_returncode && returncode != 0;

    if verbosity >= 2 || failed {
        if !stdout.is_empty() {
            print!("{}", stdout.trim_end());
            println!();
        }
        if !stderr.is_empty() {
            eprintln!("\x1b[33m{}\x1b[0m", stderr.trim_end());
        }
        eprintln!("\x1b[34m→ {}\x1b[0m", returncode);
    }

    if failed {
        bail!(
            "Command failed (exit {}): {}",
            returncode,
            argv.join(" ")
        );
    }

    Ok(ProcessResult {
        stdout,
        stderr,
        returncode,
    })
}

pub fn shell_quote(arg: &str) -> String {
    if arg
        .chars()
        .all(|c| c.is_alphanumeric() || "-.:/=_".contains(c))
    {
        arg.to_string()
    } else {
        format!("'{}'", arg.replace('\'', r"'\''"))
    }
}

/// Get the current branch name
pub fn current_branch(
    env: &HashMap<String, String>,
    verbosity: u8,
) -> Result<String> {
    let result = run_process(
        &["git", "rev-parse", "--abbrev-ref", "HEAD"],
        env,
        None,
        true,
        None,
        verbosity,
    )?;
    Ok(result.stdout.trim().to_string())
}

/// Ensure working tree is clean
pub fn ensure_clean(env: &HashMap<String, String>, verbosity: u8) -> Result<()> {
    let result = run_process(
        &["git", "status", "--porcelain"],
        env,
        None,
        true,
        None,
        verbosity,
    )?;
    if !result.stdout.trim().is_empty() {
        bail!("Repository is not clean:\n{}", result.stdout);
    }
    Ok(())
}

/// Fetch from remote
pub fn fetch(remote: &str, env: &HashMap<String, String>, verbosity: u8) -> Result<()> {
    run_process(
        &["git", "fetch", remote],
        env,
        None,
        true,
        Some(120),
        verbosity,
    )?;
    Ok(())
}

/// Checkout a detached HEAD at remote/branch
pub fn checkout_remote_branch(
    remote: &str,
    branch: &str,
    env: &HashMap<String, String>,
    verbosity: u8,
) -> Result<()> {
    let refspec = format!("{}/{}", remote, branch);
    run_process(
        &["git", "checkout", &refspec],
        env,
        None,
        true,
        None,
        verbosity,
    )?;
    Ok(())
}

/// Apply a patch via git am
pub fn git_am(
    patch_content: &str,
    env: &HashMap<String, String>,
    verbosity: u8,
) -> Result<ProcessResult> {
    run_process(
        &["git", "am", "--keep-cr", "--3way"],
        env,
        Some(patch_content),
        false,
        None,
        verbosity,
    )
}

/// Get HEAD SHA
pub fn rev_parse_head(env: &HashMap<String, String>, verbosity: u8) -> Result<String> {
    let result = run_process(
        &["git", "rev-parse", "HEAD"],
        env,
        None,
        true,
        None,
        verbosity,
    )?;
    Ok(result.stdout.trim().to_string())
}

/// Abort any in-progress git am
pub fn am_abort(env: &HashMap<String, String>) {
    let _ = run_process(
        &["git", "am", "--abort"],
        env,
        None,
        false,
        None,
        0,
    );
}

/// Hard reset
pub fn reset_hard(env: &HashMap<String, String>) {
    let _ = run_process(
        &["git", "reset", "--hard"],
        env,
        None,
        false,
        None,
        0,
    );
}

/// Checkout branch
pub fn checkout_branch(branch: &str, env: &HashMap<String, String>) {
    let _ = run_process(
        &["git", "checkout", branch],
        env,
        None,
        false,
        None,
        0,
    );
}

/// Clean working tree
pub fn clean_fxd(env: &HashMap<String, String>) {
    let _ = run_process(
        &["git", "clean", "-fxd"],
        env,
        None,
        false,
        None,
        0,
    );
}

/// Get git shortlog for reviewer lookup
pub fn shortlog_sen(
    remote_master: &str,
    env: &HashMap<String, String>,
    verbosity: u8,
) -> Result<Vec<String>> {
    let args = [
        "git",
        "-c",
        "mailmap.blob=origin/master:.mailmap",
        "shortlog",
        "-sen",
        remote_master,
    ];
    let result = run_process(&args, env, None, true, None, verbosity)?;
    Ok(result
        .stdout
        .lines()
        .map(|l| l.split('\t').nth(1).unwrap_or("").to_string())
        .collect())
}

/// Get remote URL for push
pub fn remote_get_url(
    remote: &str,
    env: &HashMap<String, String>,
    verbosity: u8,
) -> Result<String> {
    let result = run_process(
        &["git", "remote", "get-url", "--push", remote],
        env,
        None,
        true,
        None,
        verbosity,
    )?;
    Ok(result
        .stdout
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_string())
}

/// Dry-run push
pub fn push_dry_run(
    remote: &str,
    push_args: &[String],
    env: &HashMap<String, String>,
    verbosity: u8,
) -> Result<()> {
    let mut args = vec!["git", "push", "--dry-run", remote];
    let push_args_refs: Vec<&str> = push_args.iter().map(|s| s.as_str()).collect();
    args.extend_from_slice(&push_args_refs);
    run_process(&args, env, None, true, Some(120), verbosity.max(2))?;
    Ok(())
}

/// Actual push
pub fn push(
    remote: &str,
    push_args: &[String],
    env: &HashMap<String, String>,
    verbosity: u8,
) -> Result<()> {
    let mut args = vec!["git", "push", remote];
    let push_args_refs: Vec<&str> = push_args.iter().map(|s| s.as_str()).collect();
    args.extend_from_slice(&push_args_refs);
    run_process(&args, env, None, true, Some(120), verbosity.max(2))?;
    Ok(())
}

/// Push to a fork with a specific ref
pub fn push_to_fork(
    fork_remote: &str,
    sha: &str,
    branch_name: &str,
    env: &HashMap<String, String>,
    verbosity: u8,
) -> Result<()> {
    let refspec = format!("{}:refs/heads/{}", sha, branch_name);
    run_process(
        &["git", "push", fork_remote, &refspec],
        env,
        None,
        true,
        Some(120),
        verbosity,
    )?;
    Ok(())
}

/// Get git log --graph --oneline for display
pub fn log_graph_oneline(
    remote: &str,
    branch: &str,
    sha: &str,
    env: &HashMap<String, String>,
    verbosity: u8,
) -> Result<String> {
    let range = format!("{}/{}..{}", remote, branch, sha);
    let result = run_process(
        &[
            "git",
            "log",
            "--graph",
            "--oneline",
            "--abbrev=99",
            "--color=never",
            &range,
        ],
        env,
        None,
        true,
        None,
        verbosity,
    )?;
    Ok(result.stdout)
}

/// Get commit hashes for range
pub fn log_hashes(
    remote: &str,
    branch: &str,
    sha: &str,
    env: &HashMap<String, String>,
    verbosity: u8,
) -> Result<Vec<String>> {
    let range = format!("{}/{}..{}", remote, branch, sha);
    let result = run_process(
        &["git", "log", "--pretty=format:%H", &range],
        env,
        None,
        true,
        None,
        verbosity,
    )?;
    Ok(result
        .stdout
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect())
}

/// Get diffstat for display
pub fn diff_stat(
    remote: &str,
    branch: &str,
    sha: &str,
    color: &str,
    env: &HashMap<String, String>,
    verbosity: u8,
) -> Result<String> {
    let range = format!("{}/{}..{}", remote, branch, sha);
    let color_arg = format!("--color={}", color);
    let result = run_process(
        &["git", "diff", "--stat", &color_arg, &range],
        env,
        None,
        true,
        None,
        verbosity.max(2),
    )?;
    Ok(result.stdout)
}

/// Get full log for display
pub fn log_full(
    remote: &str,
    branch: &str,
    sha: &str,
    color: &str,
    env: &HashMap<String, String>,
    verbosity: u8,
) -> Result<String> {
    let range = format!("{}/{}..{}", remote, branch, sha);
    let color_arg = format!("--color={}", color);
    let result = run_process(
        &["git", "log", "--reverse", &color_arg, &range],
        env,
        None,
        true,
        None,
        verbosity.max(2),
    )?;
    Ok(result.stdout)
}

/// Launch gitk for visual inspection
pub fn gitk(
    branches: &[String],
    sha1s: &[String],
    env: &HashMap<String, String>,
) -> Result<()> {
    let mut args = vec!["gitk"];
    let branches_refs: Vec<&str> = branches.iter().map(|s| s.as_str()).collect();
    args.extend_from_slice(&branches_refs);
    let sha1s_refs: Vec<&str> = sha1s.iter().map(|s| s.as_str()).collect();
    args.extend_from_slice(&sha1s_refs);
    let _ = run_process(&args, env, None, false, None, 0);
    Ok(())
}

/// Get ISO date string for GIT_COMMITTER_DATE.
/// Returns `@<unix_seconds>` which Git accepts since git 1.8.3.
pub fn get_iso_date() -> String {
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => format!("@{}", d.as_secs()),
        Err(_) => String::new(),
    }
}
