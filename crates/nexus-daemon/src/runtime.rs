//! Which profile and page are active, brightness and blanking. Everything
//! here is plain state; the engine turns changes into frames.

use anyhow::{bail, Context, Result};
use nexus_config::{NexusConfig, Page, Profile};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Debug, Clone)]
pub(crate) struct Runtime {
    pub(crate) config: NexusConfig,
    /// File the config came from (and Studio writes); `None` for the
    /// built-in default.
    pub(crate) config_path: Option<PathBuf>,
    pub(crate) profile_index: usize,
    pub(crate) page_index: usize,
    /// Pages opened with `go_to_page`, for `go_back`.
    history: Vec<usize>,
    pub(crate) brightness: u8,
    /// Blanked through the API or CLI.
    pub(crate) manual_blank: bool,
    /// Blanked while the session is locked or the system sleeps.
    pub(crate) session_blank: bool,
    /// Off until the next touch (`display_off` action).
    pub(crate) touch_wake_blank: bool,
    pub(crate) last_touch: Instant,
    pub(crate) page_entered: Instant,
    /// Bumped on every page, profile and config change: widget indices
    /// taken under an older value point at other widgets now.
    pub(crate) generation: u64,
}

/// Direction a page change animates in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Direction {
    Forward,
    Backward,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PageChange {
    /// Profile and page id of the page left behind. Ids, not indices: a
    /// reload while it animates away may remove the page.
    pub(crate) from: (String, String),
    pub(crate) direction: Direction,
}

impl Runtime {
    pub(crate) fn load(
        path: Option<&Path>,
    ) -> Result<(Self, Option<nexus_config::MigrationReport>)> {
        let default_path = nexus_config::default_config_path();
        let path = path
            .map(Path::to_path_buf)
            .or_else(|| default_path.is_file().then_some(default_path));
        let (config, report) = match &path {
            Some(path) => nexus_config::load_and_upgrade(path)
                .with_context(|| format!("failed to load config {}", path.display()))?,
            None => (nexus_config::default_config(), None),
        };
        Ok((Self::from_config(config, path), report))
    }

    pub(crate) fn from_config(config: NexusConfig, config_path: Option<PathBuf>) -> Self {
        let profile_index = config
            .profiles
            .iter()
            .position(|profile| profile.id == config.active_profile)
            .unwrap_or(0);
        let brightness = config.profiles[profile_index].brightness.min(100);
        let mut runtime = Self {
            config,
            config_path,
            profile_index,
            page_index: 0,
            history: Vec::new(),
            brightness,
            manual_blank: false,
            session_blank: false,
            touch_wake_blank: false,
            last_touch: Instant::now(),
            page_entered: Instant::now(),
            generation: 0,
        };
        runtime.page_index = runtime.first_visible_page();
        runtime
    }

    /// Re-reads the config file, keeping the current page when it still
    /// exists.
    pub(crate) fn reload(&mut self, path: Option<&Path>) -> Result<()> {
        let path = path
            .map(Path::to_path_buf)
            .or_else(|| self.config_path.clone());
        let config = match &path {
            Some(path) => {
                nexus_config::load_and_upgrade(path)
                    .with_context(|| format!("failed to load config {}", path.display()))?
                    .0
            }
            None => nexus_config::default_config(),
        };
        let profile_id = self.profile().id.clone();
        let page_id = self.page().id.clone();
        let mut next = Self::from_config(config, path);
        if let Some(idx) = next.config.profiles.iter().position(|p| p.id == profile_id) {
            next.profile_index = idx;
            next.page_index = next.first_visible_page();
        }
        if let Some(idx) = next.profile().pages.iter().position(|p| p.id == page_id) {
            next.page_index = idx;
        }
        next.brightness = self.brightness;
        next.manual_blank = self.manual_blank;
        next.session_blank = self.session_blank;
        next.touch_wake_blank = self.touch_wake_blank;
        next.last_touch = self.last_touch;
        next.generation = self.generation + 1;
        *self = next;
        Ok(())
    }

    pub(crate) fn profile(&self) -> &Profile {
        &self.config.profiles[self.profile_index]
    }

    pub(crate) fn page(&self) -> &Page {
        &self.profile().pages[self.page_index]
    }

    pub(crate) fn settings(&self) -> &nexus_config::Settings {
        &self.config.settings
    }

    fn first_visible_page(&self) -> usize {
        self.profile()
            .pages
            .iter()
            .position(|page| !page.hidden)
            .unwrap_or(0)
    }

    /// Indices of the pages a swipe cycles through.
    pub(crate) fn visible_pages(&self) -> Vec<usize> {
        self.profile()
            .pages
            .iter()
            .enumerate()
            .filter(|(_, page)| !page.hidden)
            .map(|(idx, _)| idx)
            .collect()
    }

    /// Position of the current page among the visible ones (for the page
    /// indicator); hidden pages report the page they were opened from.
    pub(crate) fn visible_position(&self) -> (usize, usize) {
        let visible = self.visible_pages();
        let anchor = if self.page().hidden {
            self.history.last().copied().unwrap_or(self.page_index)
        } else {
            self.page_index
        };
        let pos = visible.iter().position(|idx| *idx == anchor).unwrap_or(0);
        (pos, visible.len())
    }

    /// The page a swipe in `direction` would show.
    pub(crate) fn neighbour(&self, direction: Direction) -> usize {
        let visible = self.visible_pages();
        if visible.is_empty() {
            return self.page_index;
        }
        let (pos, len) = self.visible_position();
        let next = match direction {
            Direction::Forward => (pos + 1) % len,
            Direction::Backward => (pos + len - 1) % len,
        };
        visible[next]
    }

    pub(crate) fn step(&mut self, direction: Direction) -> Option<PageChange> {
        let to = self.neighbour(direction);
        self.history.clear();
        self.set_page(to, direction)
    }

    pub(crate) fn go_to_page(&mut self, page_id: &str) -> Result<Option<PageChange>> {
        let Some(to) = self
            .profile()
            .pages
            .iter()
            .position(|page| page.id == page_id)
        else {
            bail!(
                "page '{page_id}' not found in profile '{}'",
                self.profile().id
            );
        };
        if to == self.page_index {
            return Ok(None);
        }
        self.history.push(self.page_index);
        Ok(self.set_page(to, Direction::Forward))
    }

    pub(crate) fn go_back(&mut self) -> Option<PageChange> {
        let to = self
            .history
            .pop()
            .unwrap_or_else(|| self.first_visible_page());
        self.set_page(to, Direction::Backward)
    }

    /// Profile and page id of the page showing now.
    pub(crate) fn page_key(&self) -> (String, String) {
        (self.profile().id.clone(), self.page().id.clone())
    }

    fn set_page(&mut self, to: usize, direction: Direction) -> Option<PageChange> {
        if to == self.page_index {
            return None;
        }
        let from = self.page_key();
        self.page_index = to;
        self.page_entered = Instant::now();
        self.generation += 1;
        Some(PageChange { from, direction })
    }

    pub(crate) fn switch_profile(&mut self, profile_id: &str) -> Result<()> {
        let Some(idx) = self
            .config
            .profiles
            .iter()
            .position(|profile| profile.id == profile_id)
        else {
            bail!("profile '{profile_id}' not found");
        };
        self.config.active_profile = profile_id.to_string();
        self.profile_index = idx;
        self.history.clear();
        self.page_index = self.first_visible_page();
        self.page_entered = Instant::now();
        self.generation += 1;
        self.brightness = self.profile().brightness.min(100);
        Ok(())
    }

    /// Id of the profile `direction` of the active one, wrapping around.
    pub(crate) fn neighbour_profile(&self, direction: Direction) -> &str {
        let len = self.config.profiles.len();
        let next = match direction {
            Direction::Forward => (self.profile_index + 1) % len,
            Direction::Backward => (self.profile_index + len - 1) % len,
        };
        &self.config.profiles[next].id
    }

    /// Unlocking counts as the user being back: the display timeout starts
    /// over and a timed-out strip turns on again.
    pub(crate) fn set_session_blank(&mut self, blank: bool, now: Instant) {
        if self.session_blank && !blank {
            self.last_touch = now;
            self.touch_wake_blank = false;
        }
        self.session_blank = blank;
    }

    pub(crate) fn blanked(&self) -> bool {
        self.manual_blank || self.session_blank || self.touch_wake_blank
    }

    /// Brightness to send to the panel, lowered while idle-dimmed.
    pub(crate) fn effective_brightness(&self, now: Instant) -> u8 {
        if self.idle_dimmed(now) {
            let percent = u16::from(self.config.settings.idle.dim_percent.min(100));
            (u16::from(self.brightness) * percent / 100) as u8
        } else {
            self.brightness
        }
    }

    pub(crate) fn idle_dimmed(&self, now: Instant) -> bool {
        let minutes = self.config.settings.idle.dim_after_minutes;
        minutes > 0
            && now.saturating_duration_since(self.last_touch).as_secs() >= u64::from(minutes) * 60
    }

    /// The display timeout has passed: the strip should turn off until the
    /// next touch.
    pub(crate) fn idle_off_due(&self, now: Instant) -> bool {
        let minutes = self.config.settings.idle.off_after_minutes;
        minutes > 0
            && now.saturating_duration_since(self.last_touch).as_secs() >= u64::from(minutes) * 60
    }

    /// Hidden pages with `auto_return_seconds` go back once untouched that
    /// long.
    pub(crate) fn auto_return_due(&self, now: Instant) -> bool {
        let Some(seconds) = self.page().auto_return_seconds else {
            return false;
        };
        let idle_since = self.last_touch.max(self.page_entered);
        now.saturating_duration_since(idle_since).as_secs() >= u64::from(seconds)
    }
}

/// Profile, page and brightness restored after a daemon restart.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct RuntimeState {
    pub(crate) profile_id: String,
    pub(crate) page_id: String,
    pub(crate) brightness_percent: u8,
}

impl RuntimeState {
    pub(crate) fn capture(runtime: &Runtime) -> Self {
        Self {
            profile_id: runtime.profile().id.clone(),
            page_id: runtime.page().id.clone(),
            brightness_percent: runtime.brightness,
        }
    }

    pub(crate) fn default_path() -> PathBuf {
        let state_home = std::env::var_os("XDG_STATE_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state"))
            })
            .unwrap_or_else(|| PathBuf::from("."));
        state_home.join("linexus/runtime.json")
    }

    pub(crate) fn load(path: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&text).ok()
    }

    pub(crate) fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Applies the saved state where it still matches the config.
    pub(crate) fn apply(&self, runtime: &mut Runtime) {
        if runtime.switch_profile(&self.profile_id).is_ok() {
            if let Some(idx) = runtime
                .profile()
                .pages
                .iter()
                .position(|page| page.id == self.page_id && !page.hidden)
            {
                runtime.page_index = idx;
            }
        }
        runtime.brightness = self.brightness_percent.min(100);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runtime() -> Runtime {
        Runtime::from_config(nexus_config::default_config(), None)
    }

    #[test]
    fn swipes_cycle_through_visible_pages_only() {
        let mut rt = runtime();
        rt.config.profiles[0].pages[1].hidden = true;
        assert_eq!(rt.page().id, "overview");
        rt.step(Direction::Forward).unwrap();
        assert_eq!(rt.page().id, "launcher");
        rt.step(Direction::Backward).unwrap();
        rt.step(Direction::Backward).unwrap();
        assert_eq!(rt.page().id, "focus");
    }

    #[test]
    fn go_to_page_and_back() {
        let mut rt = runtime();
        rt.config.profiles[0].pages[3].hidden = true;
        let change = rt.go_to_page("news").unwrap().unwrap();
        assert_eq!(change.direction, Direction::Forward);
        assert_eq!(rt.page().id, "news");
        // The indicator stays on the page the hidden one was opened from.
        assert_eq!(rt.visible_position().0, 0);
        let back = rt.go_back().unwrap();
        assert_eq!(back.direction, Direction::Backward);
        assert_eq!(rt.page().id, "overview");
        assert!(rt.go_to_page("missing").is_err());
    }

    #[test]
    fn idle_dimming_scales_brightness() {
        let mut rt = runtime();
        rt.brightness = 80;
        rt.config.settings.idle.dim_after_minutes = 1;
        rt.config.settings.idle.dim_percent = 25;
        let now = Instant::now();
        rt.last_touch = now;
        assert_eq!(rt.effective_brightness(now), 80);
        let later = now + std::time::Duration::from_secs(61);
        assert_eq!(rt.effective_brightness(later), 20);
    }

    #[test]
    fn display_timeout_is_due_after_the_configured_minutes() {
        let mut rt = runtime();
        let now = Instant::now();
        rt.last_touch = now;
        let later = now + std::time::Duration::from_secs(5 * 60);
        assert!(!rt.idle_off_due(later));
        rt.config.settings.idle.off_after_minutes = 5;
        assert!(!rt.idle_off_due(later - std::time::Duration::from_secs(1)));
        assert!(rt.idle_off_due(later));
    }

    #[test]
    fn profiles_cycle_in_both_directions() {
        let mut rt = runtime();
        let mut second = rt.config.profiles[0].clone();
        second.id = "second".into();
        rt.config.profiles.push(second);
        let first = rt.profile().id.clone();
        assert_eq!(rt.neighbour_profile(Direction::Forward), "second");
        assert_eq!(rt.neighbour_profile(Direction::Backward), "second");
        rt.switch_profile("second").unwrap();
        assert_eq!(rt.neighbour_profile(Direction::Forward), first);
    }

    #[test]
    fn reload_keeps_blanking_and_moves_the_generation_on() {
        let mut rt = runtime();
        rt.touch_wake_blank = true;
        rt.manual_blank = true;
        let generation = rt.generation;
        rt.reload(None).unwrap();
        assert!(rt.touch_wake_blank && rt.manual_blank);
        assert!(rt.generation > generation);
        let generation = rt.generation;
        rt.step(Direction::Forward).unwrap();
        assert!(rt.generation > generation);
    }

    #[test]
    fn saved_state_restores_profile_page_and_brightness() {
        let mut rt = runtime();
        rt.step(Direction::Forward);
        rt.brightness = 42;
        let state = RuntimeState::capture(&rt);
        let mut fresh = runtime();
        state.apply(&mut fresh);
        assert_eq!(fresh.page().id, "media");
        assert_eq!(fresh.brightness, 42);
    }
}
