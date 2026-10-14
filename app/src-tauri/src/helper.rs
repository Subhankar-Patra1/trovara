//! Trovara's helper processes, which do all the work (TRD §10.2, §11): `trovara-helper search-service` answers the
//! panel (a JSON request per line in, the reply on a line out with the request's id; lines without an id are events),
//! `trovara-helper index-service` keeps the index current (commands in, events out; with `--watch` it stays running and
//! watches the folders itself), and `trovara-helper download-model --json` fetches the model.
//!
//! Every helper is put in a job object that ends it when the app ends, however the app ends (a crash, Task Manager):
//! no helper outlives Trovara holding the GPU or the index.

use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::oneshot;

type EventSink = Arc<dyn Fn(Value) + Send + Sync>;

/// How a helper is started.
#[derive(Clone, PartialEq)]
pub struct Launch {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    /// Where its standard error goes (llama.cpp and the helper write their warnings there).
    pub log: Option<PathBuf>,
}

impl Launch {
    fn spawn(&self) -> Result<(Child, ChildStdin, ChildStdout), String> {
        let mut command = Command::new(&self.program);
        command
            .args(&self.args)
            .envs(self.env.iter().map(|(key, value)| (key, value)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(match self.log.as_ref().and_then(|path| OpenOptions::new().create(true).append(true).open(path).ok()) {
                Some(file) => Stdio::from(file),
                None => Stdio::null(),
            });
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // CREATE_NO_WINDOW: no console flashes up. The priority is the helper's own business (`--priority`): any
            // lowered priority slows its GPU work 3–8× (TRD §10.3b), so it's never set from here.
            command.creation_flags(0x0800_0000);
        }
        let mut child = command.spawn().map_err(|error| format!("can't start {}: {error}", self.program.display()))?;
        job::adopt(&child);
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        Ok((child, stdin, stdout))
    }
}

/// Asks the process to stop (`farewell` on stdin, then stdin closed), and ends it if it hasn't within `patience`.
fn stop(mut child: Child, mut stdin: ChildStdin, farewell: &str, patience: Duration) {
    let _ = writeln!(stdin, "{farewell}").and_then(|_| stdin.flush());
    drop(stdin);
    let steps = (patience.as_millis() / 50).max(1);
    for _ in 0..steps {
        if matches!(child.try_wait(), Ok(Some(_))) {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// `trovara-helper search-service`: started on the first request, and again after it died.
pub struct SearchService {
    launch: Launch,
    running: Mutex<Option<Searching>>,
    next_id: AtomicU64,
    on_event: EventSink,
}

struct Searching {
    child: Child,
    stdin: ChildStdin,
    /// Replies still to come, by request id. Each process has its own, so one that dies fails only its own requests.
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>>,
}

impl SearchService {
    pub fn new(launch: Launch, on_event: impl Fn(Value) + Send + Sync + 'static) -> SearchService {
        SearchService { launch, running: Mutex::new(None), next_id: AtomicU64::new(1), on_event: Arc::new(on_event) }
    }

    /// Sends a request and waits for its reply.
    pub async fn ask(&self, mut request: Value) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        request["id"] = json!(id);
        let reply = self.send(id, &request)?;
        let reply = reply.await.map_err(|_| "Trovara's helper stopped".to_string())?;
        match reply.get("ok") {
            Some(Value::Bool(false)) => Err(reply["error"].as_str().unwrap_or("the helper failed").to_string()),
            _ => Ok(reply),
        }
    }

    fn send(&self, id: u64, request: &Value) -> Result<oneshot::Receiver<Value>, String> {
        let mut running = self.running.lock().unwrap();
        if let Some(searching) = running.as_mut() {
            if !matches!(searching.child.try_wait(), Ok(None)) {
                *running = None;
            }
        }
        if running.is_none() {
            *running = Some(self.start()?);
        }
        let searching = running.as_mut().unwrap();
        let (sender, receiver) = oneshot::channel();
        searching.pending.lock().unwrap().insert(id, sender);
        if let Err(error) = writeln!(searching.stdin, "{request}").and_then(|_| searching.stdin.flush()) {
            searching.pending.lock().unwrap().remove(&id);
            *running = None;
            return Err(format!("Trovara's helper isn't answering: {error}"));
        }
        Ok(receiver)
    }

    fn start(&self) -> Result<Searching, String> {
        let (child, stdin, stdout) = self.launch.spawn()?;
        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>> = Arc::default();
        let (replies, on_event) = (pending.clone(), self.on_event.clone());
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let Ok(message) = serde_json::from_str::<Value>(&line) else { continue };
                match message.get("id").and_then(Value::as_u64) {
                    Some(id) => {
                        if let Some(sender) = replies.lock().unwrap().remove(&id) {
                            let _ = sender.send(message);
                        }
                    }
                    None => on_event(message),
                }
            }
            // Gone: its waiting requests fail (their senders drop here), and the next request starts another.
            replies.lock().unwrap().clear();
            on_event(json!({"event": "exit"}));
        });
        Ok(Searching { child, stdin, pending })
    }

    /// Asks it to quit, and ends it if it hasn't within `patience`.
    pub fn stop_within(&self, patience: Duration) {
        if let Some(searching) = self.running.lock().unwrap().take() {
            stop(searching.child, searching.stdin, &json!({"op": "quit"}).to_string(), patience);
        }
    }
}

/// `trovara-helper index-service --watch`: syncs, indexes, then stays running and watches the folders. `run` starts
/// it, or, when its arguments changed (other folders, other settings), starts it over with them; `send` passes it a
/// command ("sync", "pause", "resume", "full speed on"…).
pub struct IndexService {
    running: Mutex<Option<Indexing>>,
    on_event: EventSink,
    /// Bumped at each start, so the reader of a stopped helper doesn't report a newer helper's exit as its own.
    generation: Arc<AtomicU64>,
}

struct Indexing {
    child: Child,
    stdin: ChildStdin,
    launch: Launch,
}

impl IndexService {
    pub fn new(on_event: impl Fn(Value) + Send + Sync + 'static) -> IndexService {
        IndexService { running: Mutex::new(None), on_event: Arc::new(on_event), generation: Arc::default() }
    }

    /// Starts it unless it's running with these arguments already (then it's asked to sync); `restart`: start it over
    /// anyway (the model just arrived: a running helper started without it never loads it).
    pub fn run(&self, launch: &Launch, restart: bool) -> Result<(), String> {
        let mut running = self.running.lock().unwrap();
        if let Some(indexing) = running.as_mut() {
            if matches!(indexing.child.try_wait(), Ok(None)) {
                if indexing.launch == *launch && !restart {
                    return write_line(&mut indexing.stdin, "sync");
                }
                let indexing = running.take().unwrap();
                // A file being read finishes first (a long PDF step takes a few seconds).
                stop(indexing.child, indexing.stdin, "stop", Duration::from_secs(8));
            }
        }
        let (child, stdin, stdout) = launch.spawn()?;
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let (on_event, current) = (self.on_event.clone(), self.generation.clone());
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if let Ok(event) = serde_json::from_str::<Value>(&line) {
                    if current.load(Ordering::SeqCst) == generation {
                        on_event(event);
                    }
                }
            }
            if current.load(Ordering::SeqCst) == generation {
                on_event(json!({"event": "gone"}));
            }
        });
        *running = Some(Indexing { child, stdin, launch: launch.clone() });
        Ok(())
    }

    /// A command for the running helper; false when none runs.
    pub fn send(&self, command: &str) -> bool {
        let mut running = self.running.lock().unwrap();
        let Some(indexing) = running.as_mut() else { return false };
        matches!(indexing.child.try_wait(), Ok(None)) && write_line(&mut indexing.stdin, command).is_ok()
    }

    pub fn is_running(&self) -> bool {
        let mut running = self.running.lock().unwrap();
        running.as_mut().is_some_and(|indexing| matches!(indexing.child.try_wait(), Ok(None)))
    }

    /// Stops it, letting it finish the file it's reading (up to 8 s): for a restart with other settings.
    pub fn stop(&self) {
        self.stop_within(Duration::from_secs(8));
    }

    /// Asks it to stop, and ends it if it hasn't within `patience` (quitting: a file left half-read is read again
    /// next time; the index is a transaction away from consistent at any moment).
    pub fn stop_within(&self, patience: Duration) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        if let Some(indexing) = self.running.lock().unwrap().take() {
            stop(indexing.child, indexing.stdin, "stop", patience);
        }
    }
}

fn write_line(stdin: &mut ChildStdin, line: &str) -> Result<(), String> {
    writeln!(stdin, "{line}").and_then(|_| stdin.flush()).map_err(|error| error.to_string())
}

/// `trovara-helper download-model --json`: the model files, from a folder or the Hugging Face cache or downloaded with
/// resume, each checked. One at a time; `run` again while it runs does nothing. Its events go to `on_event`.
pub struct ModelDownload {
    running: Mutex<Option<Child>>,
    on_event: EventSink,
}

impl ModelDownload {
    pub fn new(on_event: impl Fn(Value) + Send + Sync + 'static) -> ModelDownload {
        ModelDownload { running: Mutex::new(None), on_event: Arc::new(on_event) }
    }

    pub fn run(&self, launch: &Launch) -> Result<(), String> {
        let mut running = self.running.lock().unwrap();
        if let Some(child) = running.as_mut() {
            if matches!(child.try_wait(), Ok(None)) {
                return Ok(());
            }
        }
        let (child, stdin, stdout) = launch.spawn()?;
        drop(stdin);
        let on_event = self.on_event.clone();
        std::thread::spawn(move || {
            let mut ended = false;
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if let Ok(event) = serde_json::from_str::<Value>(&line) {
                    ended |= event["event"] == "done" || event["event"] == "error";
                    on_event(event);
                }
            }
            if !ended {
                on_event(json!({"event": "error", "message": "the download stopped"}));
            }
        });
        *running = Some(child);
        Ok(())
    }

    pub fn stop(&self) {
        if let Some(mut child) = self.running.lock().unwrap().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Runs the helper once and returns its standard output (estimate, usn status): no window, no stdin.
pub fn run_once(program: &PathBuf, args: &[&str], env: &[(String, String)]) -> Result<String, String> {
    let mut command = Command::new(program);
    command.args(args).envs(env.iter().map(|(key, value)| (key, value))).stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let output = command.output().map_err(|error| format!("can't run {}: {error}", program.display()))?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if message.is_empty() { format!("the helper failed ({})", output.status) } else { message });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(windows)]
mod job {
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;
    use std::sync::OnceLock;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    /// The app's job: closed when the app ends (the system closes its handles), which ends every helper in it.
    struct Job(HANDLE);
    // SAFETY: a job handle is a kernel handle, usable from any thread.
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    fn job() -> Option<&'static Job> {
        static JOB: OnceLock<Option<Job>> = OnceLock::new();
        JOB.get_or_init(|| {
            // SAFETY: plain Win32 calls on a job object this function owns; the limit struct outlives the call.
            unsafe {
                let handle = CreateJobObjectW(None, None).ok()?;
                let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
                limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                SetInformationJobObject(
                    handle,
                    JobObjectExtendedLimitInformation,
                    &limits as *const _ as *const _,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
                .ok()?;
                Some(Job(handle))
            }
        })
        .as_ref()
    }

    pub fn adopt(child: &Child) {
        if let Some(job) = job() {
            // SAFETY: the child's handle is valid while `child` lives; a failure only means it isn't tied to the app.
            unsafe {
                let _ = AssignProcessToJobObject(job.0, HANDLE(child.as_raw_handle()));
            }
        }
    }
}

#[cfg(not(windows))]
mod job {
    pub fn adopt(_: &std::process::Child) {}
}
