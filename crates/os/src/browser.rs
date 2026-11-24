use std::io;

/// Opens `url` in the default browser without waiting for it.
pub fn open_url(url: &str) -> io::Result<()> {
    open::that_detached(url)
}
