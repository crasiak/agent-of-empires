use anyhow::Result;

pub fn run() -> Result<()> {
    crate::session::afk::initialize()?;
    tracing::info!("Initialized private AFK control directory");
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    #[serial_test::serial]
    fn creates_private_directory_without_enabling_any_session() {
        use std::os::unix::fs::PermissionsExt;
        let _home = crate::session::test_support::isolate_app_dir();
        super::run().unwrap();
        super::run().unwrap();
        let path = crate::session::get_app_dir().unwrap().join("afk");
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(std::fs::read_dir(path).unwrap().count(), 0);
    }
}
