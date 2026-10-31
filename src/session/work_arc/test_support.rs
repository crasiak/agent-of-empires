use crate::session::instance::{CaptureContext, SessionSidecarSource};
use crate::session::{
    ActiveExecution, ConversationBinding, ConversationProvenance, ExecutionBinding, Instance,
};
use serde_json::json;
use std::path::{Path, PathBuf};

pub(crate) static SOURCE_READS: std::sync::Mutex<Option<(PathBuf, usize)>> =
    std::sync::Mutex::new(None);

pub(super) fn observe_read(path: &Path) {
    let mut reads = SOURCE_READS.lock().unwrap();
    if let Some((_, count)) = reads.as_mut().filter(|(target, _)| target == path) {
        *count += 1;
    }
}

pub(crate) type WriteGate = (
    String,
    tokio::sync::oneshot::Sender<()>,
    std::sync::mpsc::Receiver<()>,
    std::sync::mpsc::Sender<()>,
);
pub(crate) static WRITE_GATE: std::sync::Mutex<Option<WriteGate>> = std::sync::Mutex::new(None);

pub(crate) struct WriteFinished(std::sync::mpsc::Sender<()>);
impl Drop for WriteFinished {
    fn drop(&mut self) {
        let _ = self.0.send(());
    }
}

pub(super) fn before_write(bound: &super::BoundArcInput, request: &[u8]) -> Option<WriteFinished> {
    if !serde_json::from_slice::<serde_json::Value>(request)
        .ok()
        .is_some_and(|value| value["op"] == "generate")
    {
        return None;
    }
    let gate = {
        let mut slot = WRITE_GATE.lock().unwrap();
        if slot
            .as_ref()
            .is_some_and(|(id, ..)| id == &bound.session_id)
        {
            slot.take()
        } else {
            None
        }
    };
    gate.map(|(_, entered, release, finished)| {
        let finished = WriteFinished(finished);
        let _ = entered.send(());
        let _ = release.recv_timeout(std::time::Duration::from_secs(5));
        finished
    })
}

pub(crate) type BirthGate = (
    String,
    std::sync::mpsc::Sender<()>,
    std::sync::mpsc::Receiver<()>,
    std::sync::mpsc::Sender<()>,
);
pub(crate) static BIRTH_GATE: std::sync::Mutex<Option<BirthGate>> = std::sync::Mutex::new(None);
pub(super) fn before_peer_birth(bound: &super::BoundArcInput) -> Option<WriteFinished> {
    let gate = {
        let mut slot = BIRTH_GATE.lock().unwrap();
        if slot
            .as_ref()
            .is_some_and(|(id, ..)| id == &bound.session_id)
        {
            slot.take()
        } else {
            None
        }
    };
    gate.map(|(_, entered, release, finished)| {
        let finished = WriteFinished(finished);
        let _ = entered.send(());
        let _ = release.recv_timeout(std::time::Duration::from_secs(5));
        finished
    })
}

pub(crate) struct Fixture {
    pub session: Instance,
    pub transcript: PathBuf,
    pub hooks: PathBuf,
    pub socket: PathBuf,
    pub listener: std::os::unix::net::UnixListener,
    _socket_dir: tempfile::TempDir,
}

impl Fixture {
    pub(crate) fn new(root: &Path) -> Self {
        Self::new_with_ids(
            root,
            "11111111-1111-4111-8111-111111111111",
            "22222222-2222-4222-8222-222222222222",
        )
    }

    pub(crate) fn new_with_ids(root: &Path, sid: &str, launch: &str) -> Self {
        let root = root.canonicalize().unwrap().join("synthetic-pi");
        std::fs::create_dir_all(&root).unwrap();
        let mut session = Instance::new("Synthetic work arc", root.to_str().unwrap());
        session.tool = "pi".into();
        session.source_profile = "test".into();
        let hooks = crate::hooks::ensure_instance_dir_path(&session.id)
            .unwrap()
            .canonicalize()
            .unwrap();
        let binding = ExecutionBinding {
            agent: "pi".into(),
            stores: vec![root.clone()],
            configuration: vec![],
            cwd: root.clone(),
            cwd_filesystem: "host".into(),
            filesystem: "host".into(),
            exported_default_store: None,
        };
        session.active_execution = Some(ActiveExecution {
            launch_id: launch.into(),
            binding: binding.clone(),
            capture: Some(CaptureContext::Pi {
                source: SessionSidecarSource::HostHooks(hooks.clone()),
                root: root.clone(),
            }),
            container: None,
        });
        session.lifecycle_generation = 1;
        session.set_agent_conversation(
            Some(sid.into()),
            Some(ConversationBinding {
                session_id: sid.into(),
                execution: Some(binding),
                provenance: ConversationProvenance::Observed,
                transcript_path: None,
            }),
            None,
        );
        session.work_arc_enabled = true;
        let transcript = root.join(format!("synthetic_{sid}.jsonl"));
        std::fs::write(&transcript, format!("{}\n{}\n{}\n",
            json!({"type":"session","id":sid,"cwd":root}),
            json!({"type":"message","id":"goal","parentId":null,"message":{"role":"user","content":"Synthetic goal: implement the widget"}}),
            json!({"type":"message","id":"initial","parentId":"goal","message":{"role":"assistant","content":"Proposed a plan"}}))).unwrap();
        std::fs::write(hooks.join(format!("session_id.{launch}")), sid).unwrap();
        std::fs::write(
            hooks.join(format!("session_path.{launch}")),
            transcript.to_str().unwrap(),
        )
        .unwrap();
        let socket_dir = tempfile::Builder::new()
            .prefix("arc-")
            .tempdir_in("/tmp")
            .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(socket_dir.path(), std::fs::Permissions::from_mode(0o700))
            .unwrap();
        let socket = socket_dir.path().canonicalize().unwrap().join("s");
        let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        std::fs::write(
            hooks.join(format!("work_arc_socket.{launch}")),
            socket.to_str().unwrap(),
        )
        .unwrap();
        let fixture = Self {
            session,
            transcript,
            hooks,
            socket,
            listener,
            _socket_dir: socket_dir,
        };
        fixture.publish("initial", true);
        fixture
    }

    pub(crate) fn handoff(&mut self, sid: &str) {
        let mut binding = self.session.agent_session_binding.clone().unwrap();
        binding.session_id = sid.into();
        self.session
            .set_agent_conversation(Some(sid.into()), Some(binding), None);
        let root = self.transcript.parent().unwrap();
        self.transcript = root.join(format!("synthetic_{sid}.jsonl"));
        let root = self.transcript.parent().unwrap();
        std::fs::write(&self.transcript, format!("{}\n{}\n{}\n",
            json!({"type":"session","id":sid,"cwd":root}),
            json!({"type":"message","id":"goal","parentId":null,"message":{"role":"user","content":"Synthetic goal: implement the widget"}}),
            json!({"type":"message","id":"initial","parentId":"goal","message":{"role":"assistant","content":"Successor reports progress"}}))).unwrap();
        let launch = &self.session.active_execution.as_ref().unwrap().launch_id;
        std::fs::write(self.hooks.join(format!("session_id.{launch}")), sid).unwrap();
        std::fs::write(
            self.hooks.join(format!("session_path.{launch}")),
            self.transcript.to_str().unwrap(),
        )
        .unwrap();
        self.publish("initial", true);
    }

    pub(crate) fn publish(&self, leaf: &str, settled: bool) {
        std::fs::write(self.hooks.join(format!("work_arc_cursor.{}", self.session.active_execution.as_ref().unwrap().launch_id)),
            json!({"version":1,"process_id":std::process::id(),"session_id":self.session.agent_session_id,"source":self.session.active_execution.as_ref().unwrap().launch_id,
                "leaf_id":leaf,"settled":settled,"updated_at":chrono::Utc::now()}).to_string()).unwrap();
    }

    pub(crate) fn append(&self, id: &str, parent: &str, text: &str) {
        use std::io::Write;
        writeln!(std::fs::OpenOptions::new().append(true).open(&self.transcript).unwrap(), "{}",
            json!({"type":"message","id":id,"parentId":parent,"message":{"role":"assistant","content":text}})).unwrap();
        self.publish(id, true);
    }
}
