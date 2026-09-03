use crate::domain::{AnswerResult, PromptPreset, QuestionDraft};
use crate::presets::build_prompt_with_addendum;
use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::{
    env,
    ffi::OsString,
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

pub struct CodexCliProvider {
    work_dir: PathBuf,
    configured_path: String,
    model: String,
    timeout_seconds: u64,
    prompt_addendum: String,
}

/// A model advertised by the locally installed Codex CLI.  The app-server
/// catalog is authoritative for the current account and CLI version, so the
/// UI does not need to ship a stale hard-coded model list.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexModelOption {
    pub id: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub is_default: bool,
    #[serde(default)]
    pub hidden: bool,
}

struct CodexLaunch {
    program: PathBuf,
    prefix_args: Vec<OsString>,
}

impl CodexCliProvider {
    pub fn new(
        work_dir: PathBuf,
        configured_path: String,
        model: String,
        timeout_seconds: u64,
        prompt_addendum: String,
    ) -> Self {
        Self {
            work_dir,
            configured_path,
            model,
            timeout_seconds,
            prompt_addendum,
        }
    }

    /// Queries the Codex app-server's `model/list` method.  This is the same
    /// catalog used by the interactive model picker and reflects account,
    /// rollout, and CLI-version availability.
    pub fn list_models(
        work_dir: PathBuf,
        configured_path: String,
        timeout_seconds: u64,
    ) -> Result<Vec<CodexModelOption>> {
        let launch = resolve_codex_cli(if configured_path.trim().is_empty() {
            None
        } else {
            Some(PathBuf::from(configured_path.trim()))
        })?;
        let mut command = Command::new(&launch.program);
        if env::var_os("HOME").is_none() {
            if let Some(home) = env::var_os("USERPROFILE") {
                command.env("HOME", home);
            }
        }
        command
            .current_dir(work_dir)
            .args(&launch.prefix_args)
            .args(["app-server", "--stdio"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = command
            .spawn()
            .with_context(|| format!("start Codex CLI via {}", launch.program.display()))?;
        let pid = child.id();
        let mut stdin = child
            .stdin
            .take()
            .context("Codex app-server stdin is unavailable")?;
        let stdout = child
            .stdout
            .take()
            .context("Codex app-server stdout is unavailable")?;
        let (line_tx, line_rx) = mpsc::channel::<String>();
        let reader_thread = thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines().map_while(std::result::Result::ok) {
                if line_tx.send(line).is_err() {
                    break;
                }
            }
        });
        let send = |id: u64, method: &str, params: Value, stdin: &mut std::process::ChildStdin| {
            let request = serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": method,
                "params": params,
            });
            writeln!(stdin, "{}", request)
        };
        send(
            1,
            "initialize",
            serde_json::json!({
                "clientInfo": {
                    "name": "baobao-bashi",
                    "title": "宝宝巴士",
                    "version": env!("CARGO_PKG_VERSION"),
                }
            }),
            &mut stdin,
        )
        .context("initialize Codex app-server")?;
        let initialized = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "initialized",
            "params": {},
        });
        writeln!(stdin, "{}", initialized).context("finish Codex app-server initialization")?;

        let mut request_id = 2u64;
        let mut cursor: Option<String> = None;
        let mut models = Vec::new();
        let timeout = Duration::from_secs(timeout_seconds.clamp(5, 60));
        let deadline = Instant::now() + timeout;
        loop {
            if Instant::now() >= deadline {
                terminate_process(&mut child, pid);
                let _ = reader_thread.join();
                bail!(
                    "Codex model list timed out after {} seconds",
                    timeout.as_secs()
                );
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            let line = match line_rx.recv_timeout(remaining) {
                Ok(line) => line,
                Err(error) => {
                    terminate_process(&mut child, pid);
                    let _ = reader_thread.join();
                    bail!("Codex app-server did not return a model list: {error}");
                }
            };
            let value: Value = match serde_json::from_str(&line) {
                Ok(value) => value,
                Err(_) => continue,
            };
            let Some(id) = value.get("id").and_then(Value::as_u64) else {
                continue;
            };
            if id == 1 {
                send(
                    request_id,
                    "model/list",
                    serde_json::json!({
                        "includeHidden": false,
                        "limit": 200,
                        "cursor": cursor,
                    }),
                    &mut stdin,
                )
                .context("request Codex model list")?;
                continue;
            }
            if id != request_id {
                continue;
            }
            if let Some(error) = value.get("error") {
                terminate_process(&mut child, pid);
                let _ = reader_thread.join();
                bail!("Codex model list failed: {error}");
            }
            let page: ModelListPage = serde_json::from_value(
                value
                    .get("result")
                    .cloned()
                    .context("Codex model list response has no result")?,
            )
            .context("decode Codex model list response")?;
            models.extend(page.data.into_iter().filter(|model| !model.hidden));
            if let Some(next_cursor) = page.next_cursor {
                cursor = Some(next_cursor);
                request_id += 1;
                send(
                    request_id,
                    "model/list",
                    serde_json::json!({
                        "includeHidden": false,
                        "limit": 200,
                        "cursor": cursor,
                    }),
                    &mut stdin,
                )
                .context("request next Codex model page")?;
            } else {
                break;
            }
        }
        drop(stdin);
        terminate_process(&mut child, pid);
        let _ = reader_thread.join();
        models.sort_by_key(|model| (!model.is_default, model.display_name.to_lowercase()));
        models.dedup_by(|left, right| left.id == right.id);
        Ok(models)
    }

    pub fn solve<F, G>(
        &self,
        draft: &QuestionDraft,
        preset: &PromptPreset,
        on_started: F,
        mut on_progress: G,
    ) -> Result<AnswerResult>
    where
        F: FnOnce(u32),
        G: FnMut(&str),
    {
        let prompt = build_prompt_with_addendum(preset, draft.images.len(), &self.prompt_addendum);
        let launch = resolve_codex_cli(if self.configured_path.trim().is_empty() {
            None
        } else {
            Some(PathBuf::from(self.configured_path.trim()))
        })?;
        let mut command = Command::new(&launch.program);
        // Some shells set USERPROFILE but omit HOME. Codex's configuration is
        // user-scoped, so preserve a stable home directory for its child process.
        if env::var_os("HOME").is_none() {
            if let Some(home) = env::var_os("USERPROFILE") {
                command.env("HOME", home);
            }
        }
        command
            .current_dir(&self.work_dir)
            .args(&launch.prefix_args)
            .args([
                "exec",
                "--ephemeral",
                "--json",
                "--sandbox",
                "read-only",
                "--skip-git-repo-check",
            ]);
        if !self.model.trim().is_empty() {
            command.args(["--model"]).arg(self.model.trim());
        }
        for image in &draft.images {
            command.args(["--image"]).arg(&image.path);
        }
        // Use the documented `codex exec -` form. It avoids Windows argv
        // quoting differences for a long, multilingual prompt and makes the
        // prompt boundary explicit.
        command
            .arg("-")
            .stdin(Stdio::piped())
            .stderr(Stdio::piped())
            .stdout(Stdio::piped());

        let mut child = command.spawn().with_context(|| {
            format!(
                "start Codex CLI via {}; install it and run `codex login` in a terminal first",
                launch.program.display()
            )
        })?;
        let pid = child.id();
        on_started(pid);
        if let Some(mut stdin) = child.stdin.take() {
            if let Err(error) = stdin.write_all(prompt.as_bytes()) {
                let _ = Command::new("taskkill")
                    .args(["/PID", &pid.to_string(), "/T", "/F"])
                    .output();
                bail!("send prompt to Codex CLI: {error}");
            }
        }
        let stdout = child
            .stdout
            .take()
            .context("Codex CLI stdout is unavailable")?;
        let stderr = child
            .stderr
            .take()
            .context("Codex CLI stderr is unavailable")?;
        let (line_tx, line_rx) = mpsc::channel::<String>();
        let stdout_thread = thread::spawn(move || {
            use std::io::{BufRead, BufReader};
            let reader = BufReader::new(stdout);
            let mut all = String::new();
            for line in reader.lines().map_while(std::result::Result::ok) {
                all.push_str(&line);
                all.push('\n');
                let _ = line_tx.send(line);
            }
            all
        });
        let stderr_thread = thread::spawn(move || {
            use std::io::Read;
            let mut all = String::new();
            let mut reader = stderr;
            let _ = reader.read_to_string(&mut all);
            all
        });
        let timeout = Duration::from_secs(self.timeout_seconds.clamp(5, 600));
        let deadline = Instant::now() + timeout;
        loop {
            while let Ok(line) = line_rx.try_recv() {
                on_progress(&line);
            }
            if child.try_wait().context("poll Codex CLI")?.is_some() {
                break;
            }
            if Instant::now() >= deadline {
                let _ = Command::new("taskkill")
                    .args(["/PID", &pid.to_string(), "/T", "/F"])
                    .output();
                let _ = child.wait();
                let _ = stdout_thread.join();
                let _ = stderr_thread.join();
                bail!("Codex CLI timed out after {} seconds", timeout.as_secs());
            }
            thread::sleep(Duration::from_millis(200));
        }
        let stdout_text = stdout_thread
            .join()
            .map_err(|_| anyhow::anyhow!("read Codex CLI stdout"))?;
        let stderr_text = stderr_thread
            .join()
            .map_err(|_| anyhow::anyhow!("read Codex CLI stderr"))?;
        while let Ok(line) = line_rx.try_recv() {
            on_progress(&line);
        }
        if !child
            .try_wait()
            .context("read Codex CLI exit status")?
            .map(|status| status.success())
            .unwrap_or(false)
        {
            bail!("Codex CLI failed: {}", stderr_text.trim());
        }
        parse_cli_jsonl(&stdout_text)
    }
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelListPage {
    data: Vec<CodexModelOption>,
    next_cursor: Option<String>,
}

fn terminate_process(child: &mut Child, pid: u32) {
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .output();
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Resolves Windows command shims explicitly. `Command::new("codex")` only
/// searches executable extensions on some Windows launch paths, while npm
/// commonly installs Codex as `codex.cmd`.
fn resolve_codex_cli(configured_path: Option<PathBuf>) -> Result<CodexLaunch> {
    if let Some(configured) =
        configured_path.or_else(|| env::var_os("SOLVE_LENS_CODEX_PATH").map(PathBuf::from))
    {
        let candidate = configured;
        if candidate.is_file() {
            return launch_for(candidate);
        }
        anyhow::bail!("SOLVE_LENS_CODEX_PATH does not point to a file");
    }

    let names: &[&str] = if cfg!(windows) {
        &["codex.exe", "codex.cmd", "codex.bat", "codex"]
    } else {
        &["codex"]
    };
    if let Some(path) = env::var_os("PATH") {
        for directory in env::split_paths(&path) {
            for name in names {
                let candidate = directory.join(name);
                if candidate.is_file() {
                    return launch_for(candidate);
                }
            }
        }
    }
    // Leave the final OS lookup in place for package managers or app aliases
    // that do not surface as ordinary files.
    Ok(CodexLaunch {
        program: PathBuf::from("codex"),
        prefix_args: Vec::new(),
    })
}

pub fn progress_text(line: &str) -> Option<String> {
    if let Ok(value) = serde_json::from_str::<Value>(line) {
        let mut candidates = Vec::new();
        collect_text(&value, &mut candidates);
        if let Some(text) = candidates.into_iter().last() {
            return (!text.trim().is_empty()).then_some(text);
        }
    }
    // Some CLI versions emit human-readable deltas on stdout instead of JSON.
    let text = line.trim();
    (!text.is_empty()).then_some(text.to_owned())
}

fn launch_for(candidate: PathBuf) -> Result<CodexLaunch> {
    let is_npm_cmd_shim = candidate
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("cmd"));
    if !is_npm_cmd_shim {
        return Ok(CodexLaunch {
            program: candidate,
            prefix_args: Vec::new(),
        });
    }

    // npm's Windows shim is a batch file, which CreateProcess cannot launch
    // directly. Execute its underlying Node entrypoint instead, preserving
    // prompt text as argv rather than routing it through cmd.exe parsing.
    let directory = candidate
        .parent()
        .context("Codex command shim has no parent directory")?;
    let node = directory.join("node.exe");
    let script = directory
        .join("node_modules")
        .join("@openai")
        .join("codex")
        .join("bin")
        .join("codex.js");
    if node.is_file() && script.is_file() {
        return Ok(CodexLaunch {
            program: node,
            prefix_args: vec![script.into_os_string()],
        });
    }
    anyhow::bail!(
        "Codex npm shim found at {}, but its node.exe or codex.js target is missing; reinstall @openai/codex or set SOLVE_LENS_CODEX_PATH to codex.exe",
        candidate.display()
    )
}

pub fn parse_cli_jsonl(stdout: &str) -> Result<AnswerResult> {
    let mut textual_candidates = Vec::new();
    let mut plain_lines = Vec::new();
    let mut saw_json = false;
    for line in stdout.lines().filter(|line| !line.trim().is_empty()) {
        if let Ok(value) = serde_json::from_str::<Value>(line) {
            saw_json = true;
            collect_text(&value, &mut textual_candidates);
        } else {
            plain_lines.push(line.trim().to_owned());
        }
    }
    if !saw_json && !plain_lines.is_empty() {
        return Ok(AnswerResult {
            text: plain_lines.join("\n"),
        });
    }
    if let Some(text) = textual_candidates
        .into_iter()
        .rev()
        .find(|text| !text.trim().is_empty())
    {
        return Ok(AnswerResult { text });
    }
    bail!("Codex returned no answer text")
}

fn collect_text(value: &Value, candidates: &mut Vec<String>) {
    match value {
        Value::Object(fields) => {
            for (key, child) in fields {
                if [
                    "text",
                    "message",
                    "content",
                    "output_text",
                    "delta",
                    "chunk",
                ]
                .contains(&key.as_str())
                {
                    if let Value::String(text) = child {
                        candidates.push(text.clone());
                    } else {
                        collect_text(child, candidates);
                    }
                } else {
                    collect_text(child, candidates);
                }
            }
        }
        Value::Array(values) => {
            for child in values {
                collect_text(child, candidates);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_answer_from_nested_codex_json_event() {
        let output = r#"{"type":"item.completed","item":{"text":"{\"conclusion\":\"42\",\"steps\":[\"compute\"],\"confidence\":\"high\"}"}}"#;
        let answer = parse_cli_jsonl(output).unwrap();
        assert_eq!(
            answer.text,
            "{\"conclusion\":\"42\",\"steps\":[\"compute\"],\"confidence\":\"high\"}"
        );
    }

    #[test]
    fn keeps_plain_provider_text_unchanged() {
        let answer = parse_cli_jsonl("最终答案：42\n").unwrap();
        assert_eq!(answer.text, "最终答案：42");
    }
}
