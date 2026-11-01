//! World-scoped, generational entity handles.
use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

/// Opaque process-local handle. IDs from one world are never valid in another.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Entity {
    pub(crate) world: u64,
    pub(crate) index: u32,
    pub(crate) generation: u32,
}

impl Entity {
    /// Identifier of the owning world for diagnostics; it is not a persistent save identifier.
    pub fn world(self) -> u64 {
        self.world
    }
    pub fn index(self) -> u32 {
        self.index
    }
    pub fn generation(self) -> u32 {
        self.generation
    }
}

/// Formats an opaque ID as `<world>:<slot>.<generation>` for consoles and scripts.
impl fmt::Display for Entity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}.{}", self.world, self.index, self.generation)
    }
}

impl FromStr for Entity {
    type Err = String;
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let (world, rest) = text
            .split_once(':')
            .ok_or_else(|| format!("invalid entity ID `{text}`"))?;
        let (index, generation) = rest
            .split_once('.')
            .ok_or_else(|| format!("invalid entity ID `{text}`"))?;
        Ok(Self {
            world: world
                .parse()
                .map_err(|_| format!("invalid entity world in `{text}`"))?,
            index: index
                .parse()
                .map_err(|_| format!("invalid entity slot in `{text}`"))?,
            generation: generation
                .parse()
                .map_err(|_| format!("invalid entity generation in `{text}`"))?,
        })
    }
}
