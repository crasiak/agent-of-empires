//! Additive work-arc consent, derived record and spend budget default to empty/off.

pub fn run() -> anyhow::Result<()> {
    tracing::info!("Work arc consent is off for existing Sessions");
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn legacy_session_has_no_work_arc_consent() {
        let session = crate::session::Instance::new("old", "/tmp/old");
        let mut row = serde_json::to_value(&session).unwrap();
        row.as_object_mut().unwrap().remove("work_arc_enabled");
        row.as_object_mut().unwrap().remove("work_arc");
        let restored: crate::session::Instance = serde_json::from_value(row).unwrap();
        assert!(!restored.work_arc_enabled);
        assert_eq!(restored.work_arc, Default::default());
    }
}
