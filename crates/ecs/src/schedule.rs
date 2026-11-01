//! Explicit, ordered systems with stateful closures and error propagation.
use crate::World;
use std::{error::Error, fmt};

/// A stateless, infallible system function. Stateful closures are accepted by [Schedule::add] as well.
pub type System = fn(&mut World);

type Runner = Box<dyn FnMut(&mut World) -> Result<(), String> + Send>;
struct Entry {
    name: String,
    enabled: bool,
    run: Runner,
}

/// A failure tagged with the name of the system that produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemError {
    pub system: String,
    pub message: String,
}
impl fmt::Display for SystemError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "system `{}`: {}", self.system, self.message)
    }
}
impl Error for SystemError {}

/// Runs systems serially in insertion order on the world-owning thread.
#[derive(Default)]
pub struct Schedule {
    systems: Vec<Entry>,
}
impl Schedule {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds an infallible function or stateful closure. Names identify systems for diagnostics and toggles.
    pub fn add(
        &mut self,
        name: impl Into<String>,
        mut system: impl FnMut(&mut World) + Send + 'static,
    ) {
        self.add_fallible(name, move |world| {
            system(world);
            Ok(())
        });
    }

    /// Adds a fallible system. An error stops this run before later systems execute.
    pub fn add_fallible(
        &mut self,
        name: impl Into<String>,
        system: impl FnMut(&mut World) -> Result<(), String> + Send + 'static,
    ) {
        self.systems.push(Entry {
            name: name.into(),
            enabled: true,
            run: Box::new(system),
        });
    }

    /// System names in run order; duplicate names are allowed.
    pub fn names(&self) -> Vec<&str> {
        self.systems
            .iter()
            .map(|entry| entry.name.as_str())
            .collect()
    }

    /// Enables or disables every system with this name; returns the number changed.
    pub fn set_enabled(&mut self, name: &str, enabled: bool) -> usize {
        let mut count = 0;
        for entry in self.systems.iter_mut().filter(|entry| entry.name == name) {
            if entry.enabled != enabled {
                entry.enabled = enabled;
                count += 1;
            }
        }
        count
    }

    /// Removes every system with this name and returns the number removed.
    pub fn remove(&mut self, name: &str) -> usize {
        let before = self.systems.len();
        self.systems.retain(|entry| entry.name != name);
        before - self.systems.len()
    }

    pub fn run(&mut self, world: &mut World) -> Result<(), SystemError> {
        for entry in self.systems.iter_mut().filter(|entry| entry.enabled) {
            (entry.run)(world).map_err(|message| SystemError {
                system: entry.name.clone(),
                message,
            })?;
        }
        Ok(())
    }
}
