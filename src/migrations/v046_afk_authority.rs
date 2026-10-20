use anyhow::Result;

pub fn run() -> Result<()> {
    crate::session::afk::authority::migration::run_at(&crate::session::get_app_dir()?)
}
