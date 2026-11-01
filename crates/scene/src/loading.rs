//! Script-visible state for an asset-loading transition.
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetLoadingStatus {
    #[default]
    Idle,
    Loading,
    Complete,
    Failed,
    Cancelled,
}

/// A host-owned progress snapshot. The host reports real task completion; it owns asset I/O.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AssetLoader {
    pub status: AssetLoadingStatus,
    pub phase: String,
    /// Overall normalized progress. It may be weighted across loading phases.
    pub progress: f32,
    /// Completed and total tasks within the current phase.
    pub completed: u32,
    pub total: u32,
}
impl Default for AssetLoader {
    fn default() -> Self {
        Self {
            status: AssetLoadingStatus::Idle,
            phase: String::new(),
            progress: 0.0,
            completed: 0,
            total: 0,
        }
    }
}
impl AssetLoader {
    pub fn validate(&self) -> Result<(), &'static str> {
        if !self.progress.is_finite()
            || !(0.0..=1.0).contains(&self.progress)
            || self.completed > self.total
            || self.phase.len() > 256
            || self.phase.chars().any(char::is_control)
        {
            return Err("asset loader progress, task count, or phase is invalid");
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_invalid_progress_and_task_counts() {
        assert!(AssetLoader::default().validate().is_ok());
        assert!(AssetLoader {
            progress: f32::NAN,
            ..Default::default()
        }
        .validate()
        .is_err());
        assert!(AssetLoader {
            completed: 2,
            total: 1,
            ..Default::default()
        }
        .validate()
        .is_err());
    }
}
