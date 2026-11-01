//! Append-only answer log: one JSON line per decision and per delivery, under `afk-answers/`.
use crate::session::anchored_fs::AnchoredDir;
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::Path;

pub const ANSWER_SCHEMA: &str = "aoe.afk-answer.v1";
const FILE_MAX: u64 = 4 * 1024 * 1024;
/// A decide record embeds the whole menu, `raw` included, as audit evidence. The largest menu
/// `Menu::from_input` accepts is about 22 KiB, and JSON escapes a control byte to six bytes, so the
/// worst case is about 136 KiB; this leaves margin for the rest of the record.
pub const LINE_MAX: usize = 192 * 1024;
const SAMPLES_MAX: u32 = 256;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Delivered {
    Ok,
    Failed,
    Unknown,
}

pub struct AnswerLog {
    dir: AnchoredDir,
    period: String,
}

impl AnswerLog {
    pub(crate) fn new(root: &AnchoredDir, period: &str) -> Result<AnswerLog> {
        ensure!(
            uuid::Uuid::parse_str(period).is_ok(),
            "answer log needs a period id"
        );
        let dir = root.create_child(Path::new("afk-answers"))?;
        super::super::store::private_dir(&dir)?;
        Ok(AnswerLog {
            dir,
            period: period.to_string(),
        })
    }

    fn append(&self, value: &serde_json::Value) -> Result<()> {
        let mut line = serde_json::to_vec(value)?;
        line.push(b'\n');
        ensure!(line.len() <= LINE_MAX, "answer log line too large");
        let mut file = self
            .dir
            .open_append_regular(Path::new(&format!("{}.jsonl", self.period)))?;
        ensure!(
            file.metadata()?.len() + line.len() as u64 <= FILE_MAX,
            "answer log full"
        );
        file.write_all(&line)?;
        file.sync_data()?;
        Ok(())
    }

    /// Append a decision record, stamping schema, id and time; returns the new answer id.
    pub fn decision(&self, record: &serde_json::Value) -> Result<String> {
        let id = uuid::Uuid::new_v4().to_string();
        let mut value = record.clone();
        let object = value
            .as_object_mut()
            .ok_or_else(|| anyhow::anyhow!("answer record must be an object"))?;
        object.insert("schema".into(), ANSWER_SCHEMA.into());
        object.insert("answer_id".into(), id.clone().into());
        object.insert("ts".into(), chrono::Utc::now().to_rfc3339().into());
        object.entry("delivered").or_insert("pending".into());
        self.append(&value)?;
        Ok(id)
    }

    pub fn delivered(&self, answer_id: &str, status: Delivered) -> Result<()> {
        ensure!(
            uuid::Uuid::parse_str(answer_id).is_ok(),
            "invalid answer id"
        );
        self.append(&serde_json::json!({
            "schema": ANSWER_SCHEMA,
            "answer_id": answer_id,
            "ts": chrono::Utc::now().to_rfc3339(),
            "delivered": status,
        }))
    }

    /// Keep text no format recognised, for writing a new format; returns the sample's file name.
    pub fn unrecognised(&self, raw: &str) -> Result<String> {
        let mut end = raw.len().min(super::menu::RAW_MAX);
        while !raw.is_char_boundary(end) {
            end -= 1;
        }
        for n in 1..=SAMPLES_MAX {
            let name = format!("{}-unrecognised-{n}.txt", self.period);
            if let Some(mut file) = self.dir.create_new_regular(Path::new(&name))? {
                file.write_all(&raw.as_bytes()[..end])?;
                file.sync_data()?;
                return Ok(name);
            }
        }
        anyhow::bail!("unrecognised sample capacity reached")
    }
}
