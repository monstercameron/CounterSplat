//! Explicit surface presentation mode selection.
//!
//! Native hosts should configure the selected mode only when no surface frame is in flight.
//! `Immediate` is required for true VSync-off: a silent FIFO or mailbox fallback would misreport it.

/// User-requested presentation policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vsync {
    On,
    Off,
}

/// Pick a mode actually advertised by the surface; never claim unsupported VSync-off is active.
pub fn select_mode(
    vsync: Vsync,
    supported: &[wgpu::PresentMode],
) -> Result<wgpu::PresentMode, &'static str> {
    let requested = match vsync {
        Vsync::On => wgpu::PresentMode::Fifo,
        Vsync::Off => wgpu::PresentMode::Immediate,
    };
    if supported.contains(&requested) {
        Ok(requested)
    } else {
        match vsync {
            Vsync::On => Err("surface does not advertise FIFO presentation"),
            Vsync::Off => Err("VSync off is unavailable on this surface"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selects_only_genuine_supported_modes() {
        use wgpu::PresentMode::{Fifo, Immediate, Mailbox};
        assert_eq!(select_mode(Vsync::On, &[Fifo, Immediate]), Ok(Fifo));
        assert_eq!(select_mode(Vsync::Off, &[Fifo, Immediate]), Ok(Immediate));
        assert!(select_mode(Vsync::Off, &[Fifo, Mailbox]).is_err());
        assert!(select_mode(Vsync::On, &[]).is_err());
    }
}
