//! A named workspace: a layout tree plus floating windows.

use crate::geometry::{MonitorId, Orientation, WindowId};
use crate::tree::{Container, Layout};

#[derive(Debug, Clone)]
pub(crate) struct Workspace {
    pub root: Container,
    pub floating: Vec<WindowId>,
    /// Windows in the order they were focused, most recent last.
    pub mru: Vec<WindowId>,
    /// The monitor this workspace was last shown on.
    pub last_monitor: Option<MonitorId>,
    pub fullscreen: Option<WindowId>,
}

impl Workspace {
    pub fn new(layout: Layout) -> Self {
        Self {
            root: Container::new(Orientation::H, layout),
            floating: Vec::new(),
            mru: Vec::new(),
            last_monitor: None,
            fullscreen: None,
        }
    }

    pub fn focused(&self) -> Option<WindowId> {
        self.mru.last().copied()
    }

    /// Marks a window as the most recently focused one.
    pub fn touch(&mut self, id: WindowId) {
        self.mru.retain(|w| *w != id);
        self.mru.push(id);
    }

    /// The most recently focused window that is tiled.
    pub fn last_tiled(&self) -> Option<WindowId> {
        self.mru
            .iter()
            .rev()
            .copied()
            .find(|w| self.root.contains(*w))
    }

    /// Removes every trace of a window from this workspace.
    pub fn forget(&mut self, id: WindowId) {
        self.root.remove(id);
        self.floating.retain(|w| *w != id);
        self.mru.retain(|w| *w != id);
        if self.fullscreen == Some(id) {
            self.fullscreen = None;
        }
    }

    pub fn is_empty(&self) -> bool {
        self.root.is_empty() && self.floating.is_empty()
    }

    pub fn windows(&self) -> impl Iterator<Item = WindowId> + '_ {
        self.root
            .windows()
            .into_iter()
            .chain(self.floating.iter().copied())
    }
}
