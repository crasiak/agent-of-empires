use anyhow::Result;

pub fn run() -> Result<()> {
    crate::session::afk::delegation::initialize_settlement()?;
    tracing::info!(
        "Initialized AFK protocol-3 settlement namespace; explicit reenrollment required"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    #[serial_test::serial]
    fn preserves_v2_evidence_without_new_authority() {
        let _home = crate::session::test_support::isolate_app_dir();
        super::super::v037_afk_runtime::run().unwrap();
        let app = crate::session::get_app_dir().unwrap();
        std::fs::write(app.join("afk-runtime-v2/evidence"), b"old grant").unwrap();
        super::run().unwrap();
        super::run().unwrap();
        assert_eq!(
            std::fs::read(app.join("afk-runtime-v2/evidence")).unwrap(),
            b"old grant"
        );
        assert_eq!(
            std::fs::read_dir(app.join("afk-runtime-v3"))
                .unwrap()
                .count(),
            0
        );
    }
}
