//! Collapses search hits that belong to the same thread into one row.

use std::collections::HashMap;

use super::query::{Hit, Session};

pub struct Row {
    pub best: Hit,
    /// Remaining hits of the same thread, in rank order.
    pub others: Vec<Hit>,
}

/// Groups hits by `thread` (a key such as the hosting pane; hits it gives
/// `None` stay alone); a row sits where its best-ranked hit was.
pub fn group(
    hits: Vec<Hit>,
    max_rows: usize,
    thread: &dyn Fn(&Session) -> Option<String>,
) -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    let mut by_thread: HashMap<String, usize> = HashMap::new();
    for hit in hits {
        let key = thread(&hit.session);
        let existing = key.as_ref().and_then(|k| by_thread.get(k).copied());
        match existing {
            Some(idx) => rows[idx].others.push(hit),
            None if rows.len() < max_rows => {
                if let Some(key) = key {
                    by_thread.insert(key, rows.len());
                }
                rows.push(Row {
                    best: hit,
                    others: Vec::new(),
                });
            }
            None => {}
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_search::query::parse;

    fn fixture() -> Vec<Hit> {
        parse(include_bytes!("testdata/query_v1.json")).unwrap()
    }

    fn by_run(s: &Session) -> Option<String> {
        s.run_id.clone()
    }

    #[test]
    fn groups_by_thread_keeping_rank_order() {
        let rows = group(fixture(), 20, &by_run);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].best.session.session_key, "L1");
        assert_eq!(rows[1].best.session.session_key, "L2");
        assert_eq!(
            rows[1]
                .others
                .iter()
                .map(|h| h.session.session_key.as_str())
                .collect::<Vec<_>>(),
            ["L2b"]
        );
        assert_eq!(group(fixture(), 1, &by_run).len(), 1);
        // Another key, such as the hosting pane, regroups the same hits.
        assert_eq!(group(fixture(), 20, &|_| Some("pane".into())).len(), 1);
    }

    #[test]
    fn hits_without_run_id_stay_separate() {
        let mut hits = fixture();
        for hit in &mut hits {
            hit.session.run_id = None;
        }
        let rows = group(hits, 20, &by_run);
        assert_eq!(rows.len(), 3);
        assert!(rows.iter().all(|r| r.others.is_empty()));
    }
}
