//! Stable identifiers for topology entities.
//!
//! IDs are process-unique `u64`s handed out by [`IdGen`]. They are deliberately
//! opaque so that callers (the GUI, the control socket, the CLI) refer to panes
//! and tabs by identity rather than by position, mirroring upstream cmux's
//! `workspace:1` / `surface:1` addressing.

use serde::{Deserialize, Serialize};

macro_rules! define_id {
    ($name:ident, $prefix:literal) => {
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub u64);

        impl $name {
            pub const fn raw(self) -> u64 {
                self.0
            }
        }

        impl std::fmt::Debug for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, concat!($prefix, ":{}"), self.0)
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, concat!($prefix, ":{}"), self.0)
            }
        }
    };
}

define_id!(WorkspaceId, "workspace");
define_id!(TabId, "tab");
define_id!(PaneId, "surface");

/// Monotonic id generator. One per [`crate::AppState`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdGen {
    next: u64,
}

impl Default for IdGen {
    fn default() -> Self {
        // Start at 1 so `0` can stay reserved / "none" in any future encoding.
        Self { next: 1 }
    }
}

impl IdGen {
    fn bump(&mut self) -> u64 {
        let v = self.next;
        self.next += 1;
        v
    }

    pub fn workspace(&mut self) -> WorkspaceId {
        WorkspaceId(self.bump())
    }
    pub fn tab(&mut self) -> TabId {
        TabId(self.bump())
    }
    pub fn pane(&mut self) -> PaneId {
        PaneId(self.bump())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_monotonic() {
        let mut g = IdGen::default();
        let a = g.workspace();
        let b = g.tab();
        let c = g.pane();
        assert_eq!((a.raw(), b.raw(), c.raw()), (1, 2, 3));
    }

    #[test]
    fn display_uses_addressing_prefix() {
        assert_eq!(WorkspaceId(7).to_string(), "workspace:7");
        assert_eq!(PaneId(3).to_string(), "surface:3");
    }
}
