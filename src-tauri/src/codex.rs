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
    sync::{mpsc, Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

/// Handle retained by the application while an app-server turn is running.
/// It deliberately owns only the control channel, not the conversation: Codex
/// persists the thread and a new app-server process can resume it after restart.
pub struct InteractiveTurnControl {
    stdin: Arc<Mutex<Option<std::process::ChildStdin>>>,
    turn_id: Mutex<Option<String>>,
}

impl InteractiveTurnControl {
    pub fn interrupt(&self) -> Result<()> {
        let turn_id = self.turn_id.lock().expect("turn id lock").clone()
            .context("Codex turn has not started yet")?;
        let mut stdin = self.stdin.lock().expect("app-server stdin lock");
        let stdin = stdin.as_mut().context("Codex app-server is no longer connected")?;
        writeln!(stdin, "{}", serde_json::json!({
            "jsonrpc":"2.0", "id": 9_999_u64, "method":"turn/interrupt",
            "params": {"turnId": turn_id}
        })).context("send turn/interrupt")?;
        stdin.flush().context("flush turn/interrupt")
    }
}

pub struct InteractiveCodexProvider {
    inner: CodexCliProvider,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexThreadOption {
    pub id: String,
    #[serde(default)] pub name: String,
    #[serde(default)] pub model: String,
    #[serde(default)] pub updated_at: String,
    #[serde(default)] pub archived: bool,
}

impl InteractiveCodexProvider {
    pub fn new(
        work_dir: PathBuf, configured_path: String, model: String, reasoning_effort: String,
        service_tier: String, timeout_seconds: u64, prompt_addendum: String,
    ) -> Self {
        Self { inner: CodexCliProvider::new(work_dir, configured_path, model, reasoning_effort, service_tier, timeout_seconds, prompt_addendum) }
    }

    pub fn list_threads(&self, include_archived: bool) -> Result<Vec<CodexThreadOption>> {
        let (mut child, stdin, rx, reader) = self.start_server()?; let pid = child.id();
        let control = Arc::new(InteractiveTurnControl { stdin: Arc::new(Mutex::new(Some(stdin))), turn_id: Mutex::new(None) });
        let deadline = Instant::now() + Duration::from_secs(self.inner.timeout_seconds.clamp(5, 60));
        self.request(&control, 1, "initialize", serde_json::json!({"clientInfo":{"name":"baobao-bashi","version":env!("CARGO_PKG_VERSION")}}))?;
        self.wait_response(&rx, 1, deadline)?; self.notify_initialized(&control)?;
        self.request(&control, 2, "thread/list", serde_json::json!({"limit":100,"archived":include_archived}))?;
        let response = self.wait_response(&rx, 2, deadline)?;
        *control.stdin.lock().expect("app-server stdin lock") = None; terminate_process(&mut child, pid); let _ = reader.join();
        let data = response.pointer("/result/data").and_then(Value::as_array)
            .or_else(|| response.pointer("/result/threads").and_then(Value::as_array))
            .cloned().unwrap_or_default();
        // Thread metadata has changed slightly between CLI releases (for
        // example updatedAt may be an ISO string or a numeric timestamp).
        // Normalize only the stable identity/display fields instead of
        // deserializing the entire server object strictly.
        Ok(data.into_iter().filter_map(|item| {
            let object = item.as_object()?;
            let id = object.get("id").and_then(Value::as_str)
                .or_else(|| object.get("threadId").and_then(Value::as_str))?.to_owned();
            let name = object.get("name").and_then(Value::as_str)
                .or_else(|| object.get("title").and_then(Value::as_str))
                .or_else(|| object.get("preview").and_then(Value::as_str)).unwrap_or("").to_owned();
            let model = object.get("model").and_then(Value::as_str).unwrap_or("").to_owned();
            let updated_at = object.get("updatedAt").or_else(|| object.get("updated_at"))
                .map(|value| value.as_str().map(str::to_owned).unwrap_or_else(|| value.to_string())).unwrap_or_default();
            let archived = object.get("archived").and_then(Value::as_bool).unwrap_or(false);
            Some(CodexThreadOption { id, name, model, updated_at, archived })
        }).collect())
    }

    pub fn solve<F, G, H>(
        &self, existing_thread: Option<String>, strict_resume: bool, draft: &QuestionDraft, preset: &PromptPreset,
        on_started: F, mut on_thread: G, mut on_control: H,
    ) -> Result<AnswerResult>
    where F: FnOnce(u32), G: FnMut(String), H: FnMut(Arc<InteractiveTurnControl>) {
        let (mut child, stdin, rx, reader) = self.start_server()?;
        let pid = child.id();
        on_started(pid);
        let control = Arc::new(InteractiveTurnControl { stdin: Arc::new(Mutex::new(Some(stdin))), turn_id: Mutex::new(None) });
        let deadline = Instant::now() + Duration::from_secs(self.inner.timeout_seconds.clamp(5, 600));
        let mut request_id = 2_u64;
        self.request(&control, 1, "initialize", serde_json::json!({"clientInfo":{"name":"baobao-bashi","title":"宝宝巴士","version":env!("CARGO_PKG_VERSION")}}))?;
        self.wait_response(&rx, 1, deadline)?;
        self.notify_initialized(&control)?;
        let thread_id = if let Some(id) = existing_thread {
            self.request(&control, request_id, "thread/resume", serde_json::json!({"threadId":id}))?;
            match self.wait_response(&rx, request_id, deadline) {
                Ok(_) => id,
                Err(error) => { if strict_resume { return Err(error); } request_id += 1; self.start_thread(&control, &rx, request_id, deadline)? }
            }
        } else { self.start_thread(&control, &rx, request_id, deadline)? };
        on_thread(thread_id.clone());
        request_id += 1;
        let prompt = build_prompt_with_addendum(preset, draft.images.len(), &self.inner.prompt_addendum);
        let mut input = vec![serde_json::json!({"type":"text","text":prompt})];
        input.extend(draft.images.iter().map(|image| serde_json::json!({"type":"localImage","path":image.path})));
        let mut params = serde_json::json!({"threadId":thread_id,"input":input});
        if !self.inner.model.trim().is_empty() { params["model"] = Value::String(self.inner.model.trim().to_owned()); }
        // Publish the control before requesting the turn so a fast user cancel
        // never falls back to force-killing an app-server process.
        on_control(control.clone());
        self.request(&control, request_id, "turn/start", params)?;
        let started = self.wait_response(&rx, request_id, deadline)?;
        if let Some(turn_id) = started.pointer("/result/turn/id").and_then(Value::as_str) {
            *control.turn_id.lock().expect("turn id lock") = Some(turn_id.to_owned());
        }
        let mut text = String::new();
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() { terminate_process(&mut child, pid); bail!("Codex app-server turn timed out"); }
            let line = rx.recv_timeout(remaining).context("Codex app-server disconnected while waiting for turn completion")?;
            let value: Value = serde_json::from_str(&line).context("decode Codex app-server JSON-RPC message")?;
            match value.get("method").and_then(Value::as_str) {
                Some("turn/started") => if let Some(id) = value.pointer("/params/turn/id").and_then(Value::as_str) { *control.turn_id.lock().expect("turn id lock") = Some(id.to_owned()); },
                Some("item/agentMessage/delta") => {
                    if let Some(delta) = value.pointer("/params/delta").and_then(Value::as_str) { text.push_str(delta); }
                    else if let Some(delta) = value.pointer("/params/delta/text").and_then(Value::as_str) { text.push_str(delta); }
                }
                Some("turn/completed") => {
                    let status = value.pointer("/params/turn/status").and_then(Value::as_str).unwrap_or("unknown");
                    *control.stdin.lock().expect("app-server stdin lock") = None;
                    terminate_process(&mut child, pid); let _ = reader.join();
                    if matches!(status, "completed" | "success") && !text.trim().is_empty() { return Ok(AnswerResult { text }); }
                    bail!("Codex turn completed with status {status}");
                }
                _ => {}
            }
        }
    }

    pub fn archive(&self, thread_id: &str) -> Result<()> {
        let (mut child, stdin, rx, reader) = self.start_server()?; let pid = child.id();
        let control = Arc::new(InteractiveTurnControl { stdin: Arc::new(Mutex::new(Some(stdin))), turn_id: Mutex::new(None) });
        let deadline = Instant::now() + Duration::from_secs(self.inner.timeout_seconds.clamp(5, 60));
        self.request(&control, 1, "initialize", serde_json::json!({"clientInfo":{"name":"baobao-bashi","version":env!("CARGO_PKG_VERSION")}}))?;
        self.wait_response(&rx, 1, deadline)?; self.notify_initialized(&control)?;
        self.request(&control, 2, "thread/resume", serde_json::json!({"threadId":thread_id}))?; self.wait_response(&rx, 2, deadline)?;
        self.request(&control, 3, "thread/archive", serde_json::json!({"threadId":thread_id}))?; self.wait_response(&rx, 3, deadline)?;
        *control.stdin.lock().expect("app-server stdin lock") = None; terminate_process(&mut child, pid); let _ = reader.join(); Ok(())
    }

    fn start_server(&self) -> Result<(Child, std::process::ChildStdin, mpsc::Receiver<String>, thread::JoinHandle<()>)> {
        let launch = resolve_codex_cli(if self.inner.configured_path.trim().is_empty() { None } else { Some(PathBuf::from(self.inner.configured_path.trim())) })?;
        let mut command = Command::new(&launch.program); if env::var_os("HOME").is_none() { if let Some(home) = env::var_os("USERPROFILE") { command.env("HOME", home); } }
        command.current_dir(&self.inner.work_dir).args(&launch.prefix_args).args(["app-server", "--stdio"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        let mut child = command.spawn().context("start Codex app-server")?;
        let stdin = child.stdin.take().context("Codex app-server stdin is unavailable")?;
        let stdout = child.stdout.take().context("Codex app-server stdout is unavailable")?;
        let (tx, rx) = mpsc::channel(); let reader = thread::spawn(move || { for line in BufReader::new(stdout).lines().map_while(std::result::Result::ok) { if tx.send(line).is_err() { break; } } });
        Ok((child, stdin, rx, reader))
    }
    fn request(&self, control: &InteractiveTurnControl, id: u64, method: &str, params: Value) -> Result<()> { let mut lock = control.stdin.lock().expect("app-server stdin lock"); let stdin = lock.as_mut().context("Codex app-server is no longer connected")?; writeln!(stdin, "{}", serde_json::json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))?; stdin.flush()?; Ok(()) }
    fn notify_initialized(&self, control: &InteractiveTurnControl) -> Result<()> { let mut lock = control.stdin.lock().expect("app-server stdin lock"); let stdin = lock.as_mut().context("Codex app-server is no longer connected")?; writeln!(stdin, "{}", serde_json::json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?; stdin.flush()?; Ok(()) }
    fn wait_response(&self, rx: &mpsc::Receiver<String>, id: u64, deadline: Instant) -> Result<Value> { loop { let line = rx.recv_timeout(deadline.saturating_duration_since(Instant::now())).context("Codex app-server response timed out")?; let value: Value = serde_json::from_str(&line).context("decode Codex app-server JSON-RPC response")?; if value.get("id").and_then(Value::as_u64) != Some(id) { continue; } if let Some(error) = value.get("error") { bail!("Codex app-server request failed: {error}"); } return Ok(value); } }
    fn start_thread(&self, control: &InteractiveTurnControl, rx: &mpsc::Receiver<String>, id: u64, deadline: Instant) -> Result<String> {
        let mut params = serde_json::json!({});
        if !self.inner.model.trim().is_empty() { params["model"] = Value::String(self.inner.model.trim().to_owned()); }
        self.request(control, id, "thread/start", params)?;
        let response = self.wait_response(rx, id, deadline)?;
        response.pointer("/result/thread/id").and_then(Value::as_str).map(str::to_owned).context("thread/start response has no thread id")
    }
}

pub struct CodexCliProvider {
    work_dir: PathBuf,
    configured_path: String,
    model: String,
    reasoning_effort: String,
    service_tier: String,
    timeout_seconds: u64,
    prompt_addendum: String,
}

/// A model advertised by the locally installed Codex CLI.  The app-server
/// catalog is authoritative for the current account and CLI version, so the
/// UI does not need to ship a stale hard-coded model list.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexReasoningOption {
    pub reasoning_effort: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexServiceTierOption {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
}

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
    #[serde(default)]
    pub supported_reasoning_efforts: Vec<CodexReasoningOption>,
    #[serde(default)]
    pub default_reasoning_effort: String,
    #[serde(default)]
    pub additional_speed_tiers: Vec<String>,
    #[serde(default)]
    pub service_tiers: Vec<CodexServiceTierOption>,
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
        reasoning_effort: String,
        service_tier: String,
        timeout_seconds: u64,
        prompt_addendum: String,
    ) -> Self {
        Self {
            work_dir,
            configured_path,
            model,
            reasoning_effort,
            service_tier,
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
        // Codex CLI exposes these controls through its generic --config
        // override. Keeping them per invocation prevents the app from
        // mutating the user's global ~/.codex/config.toml.
        if !self.reasoning_effort.trim().is_empty() {
            command.args(["--config"]).arg(config_override(
                "model_reasoning_effort",
                &self.reasoning_effort,
            ));
        }
        if !self.service_tier.trim().is_empty() {
            command
                .args(["--config"])
                .arg(config_override("service_tier", &self.service_tier));
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

fn config_override(key: &str, value: &str) -> String {
    // Values are validated by AppConfig before a solve starts. Strip quotes as
    // an extra guard because --config parses the value as TOML.
    let sanitized = value.trim().replace('"', "");
    format!(r#"{key}="{sanitized}""#)
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
    if let Some(configured) = configured_path.or_else(|| {
        env::var_os("BAOBAO_BASHI_CODEX_PATH")
            .or_else(|| env::var_os("SOLVE_LENS_CODEX_PATH"))
            .map(PathBuf::from)
    }) {
        let candidate = configured;
        if candidate.is_file() {
            return launch_for(candidate);
        }
        anyhow::bail!("BAOBAO_BASHI_CODEX_PATH does not point to a file");
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

#[allow(dead_code)]
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
        "Codex npm shim found at {}, but its node.exe or codex.js target is missing; reinstall @openai/codex or set BAOBAO_BASHI_CODEX_PATH to codex.exe",
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

    #[test]
    fn formats_codex_config_override_as_toml_string() {
        assert_eq!(
            config_override("service_tier", "priority"),
            r#"service_tier="priority""#
        );
        assert_eq!(
            config_override("model_reasoning_effort", "high\""),
            r#"model_reasoning_effort="high""#
        );
    }
}
