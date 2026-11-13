//! A display-free startup check used before replacing an installed Linux binary.
//! Unlike filter/model tests, this initializes the actual linked mpv core and
//! validates its client API version. It never opens video, audio, or a window.
use anyhow::{Context, Result};

pub fn check_runtime() -> Result<()> {
    let _mpv = libmpv2::Mpv::with_initializer(|init| {
        init.set_property("config", "no")?;
        init.set_property("terminal", "no")?;
        init.set_property("vo", "null")?;
        init.set_property("ao", "null")?;
        Ok(())
    })
    .context("The mpv runtime is incompatible. Linux requires libmpv.so.2 (mpv 0.35 or newer).")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn linked_mpv_core_initializes_with_supported_client_api() {
        super::check_runtime().unwrap();
    }
}
