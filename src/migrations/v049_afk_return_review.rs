pub fn run() -> anyhow::Result<()> {
    crate::session::afk::review::annotations::initialize(&crate::session::get_app_dir()?)
}
