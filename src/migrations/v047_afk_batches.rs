use anyhow::Result;

pub fn run() -> Result<()> {
    crate::session::afk::authority::migration_v2::run_at(&crate::session::get_app_dir()?)
}
