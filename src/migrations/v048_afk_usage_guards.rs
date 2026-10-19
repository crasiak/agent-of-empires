pub fn run() -> anyhow::Result<()> {
    let app = crate::session::get_app_dir()?;
    crate::session::afk::authority::migration_v3::run_at(&app)?;
    crate::session::afk::wake::initialize(&app)
}
