use anyhow::Result;

pub fn run() -> Result<()> {
    crate::session::afk::delegation::initialize()?;
    tracing::info!(
        "Initialized private AFK protocol-2 runtime namespace; legacy windows are not adopted"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    #[serial_test::serial]
    fn preserves_legacy_evidence_without_enrollment() {
        let _home = crate::session::test_support::isolate_app_dir();
        super::super::v036_afk_control::run().unwrap();
        let app = crate::session::get_app_dir().unwrap();
        std::fs::write(app.join("afk/evidence"), b"legacy").unwrap();
        super::run().unwrap();
        super::run().unwrap();
        assert_eq!(std::fs::read(app.join("afk/evidence")).unwrap(), b"legacy");
        assert_eq!(
            std::fs::read_dir(app.join("afk-runtime-v2"))
                .unwrap()
                .count(),
            0
        );
    }
}
