pub mod matcher;
pub use matcher::search;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Application {
    pub name: String,
    pub desktop_id: String,
    pub icon: Option<String>,
}

/// An open window surfaced by the GNOME Shell extension over D-Bus. `app_name`
/// and `icon` come from `Shell.WindowTracker`, which can return null for system
/// windows; both are `Option<String>` and the extension coerces empty strings
/// to `None` on the Rust side (see `app/gnome/src/windows.rs`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Window {
    pub id: u64,
    pub title: String,
    pub app_name: Option<String>,
    pub icon: Option<String>,
    pub workspace: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EntryKind {
    Application,
    Window,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    Application(Application),
    Window(Window),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", content = "id", rename_all = "snake_case")]
pub enum EntryRef {
    Application(String),
    Window(u64),
}

impl Entry {
    pub fn name(&self) -> &str {
        match self {
            Entry::Application(app) => app.name.as_str(),
            Entry::Window(w) => w.title.as_str(),
        }
    }

    pub fn icon(&self) -> Option<&str> {
        match self {
            Entry::Application(app) => app.icon.as_deref(),
            Entry::Window(w) => w.icon.as_deref(),
        }
    }

    pub fn kind(&self) -> EntryKind {
        match self {
            Entry::Application(_) => EntryKind::Application,
            Entry::Window(_) => EntryKind::Window,
        }
    }

    pub fn reference(&self) -> EntryRef {
        match self {
            Entry::Application(app) => EntryRef::Application(app.desktop_id.clone()),
            Entry::Window(w) => EntryRef::Window(w.id),
        }
    }
}

pub fn resolve<'a>(entries: &'a [Entry], reference: &EntryRef) -> Option<&'a Entry> {
    entries.iter().find(|e| &e.reference() == reference)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_application(name: &str, desktop_id: &str, icon: Option<&str>) -> Application {
        Application {
            name: name.to_string(),
            desktop_id: desktop_id.to_string(),
            icon: icon.map(str::to_string),
        }
    }

    fn make_window(id: u64, title: &str, app_name: Option<&str>, icon: Option<&str>) -> Window {
        Window {
            id,
            title: title.to_string(),
            app_name: app_name.map(str::to_string),
            icon: icon.map(str::to_string),
            workspace: 0,
        }
    }

    #[test]
    fn entry_reference_round_trips_application() {
        let app = make_application("Firefox", "firefox.desktop", Some("firefox"));
        let entry = Entry::Application(app.clone());

        let reference = entry.reference();
        assert_eq!(
            reference,
            EntryRef::Application(app.desktop_id.clone()),
            "entry.reference() should be EntryRef::Application(desktop_id); got {reference:?}"
        );

        let entries = vec![entry.clone()];
        let resolved = resolve(&entries, &entry.reference());
        assert!(
            matches!(resolved, Some(r) if r == &entry),
            "resolve should return Some(&entry) for its own reference; got {resolved:?}"
        );
    }

    #[test]
    fn resolve_finds_application_by_reference() {
        let entries = vec![
            Entry::Application(make_application("Alpha", "alpha.desktop", None)),
            Entry::Application(make_application("Beta", "beta.desktop", None)),
            Entry::Application(make_application("Gamma", "gamma.desktop", None)),
        ];

        let reference = EntryRef::Application("beta.desktop".into());
        let resolved = resolve(&entries, &reference);

        let found = resolved.expect("resolve should find an entry for beta.desktop");
        assert_eq!(
            found.name(),
            "Beta",
            "resolve should return the Beta entry, not the first; got {:?}",
            found.name()
        );
    }

    #[test]
    fn resolve_returns_none_for_missing_reference() {
        let entries = vec![
            Entry::Application(make_application("Alpha", "alpha.desktop", None)),
            Entry::Application(make_application("Beta", "beta.desktop", None)),
            Entry::Application(make_application("Gamma", "gamma.desktop", None)),
        ];

        let missing = EntryRef::Application("missing.desktop".into());
        assert_eq!(
            resolve(&entries, &missing),
            None,
            "resolve should return None for a desktop_id not in the slice"
        );

        let empty: [Entry; 0] = [];
        let anything = EntryRef::Application("anything.desktop".into());
        assert_eq!(
            resolve(&empty, &anything),
            None,
            "resolve over an empty slice should always return None"
        );
    }

    #[test]
    fn entry_ref_serializes_to_tagged_json() {
        let r = EntryRef::Application("firefox.desktop".into());

        let serialized = serde_json::to_string(&r).expect("EntryRef should serialize to JSON");
        assert_eq!(
            serialized, r#"{"type":"application","id":"firefox.desktop"}"#,
            "EntryRef should serialize with tag=type/content=id and snake_case variant; got {serialized}"
        );

        let round_tripped: EntryRef =
            serde_json::from_str(&serialized).expect("EntryRef should deserialize from JSON");
        assert_eq!(
            round_tripped, r,
            "EntryRef should round-trip via serde_json; got {round_tripped:?}"
        );
    }

    #[test]
    fn entry_methods_return_application_data() {
        let app = make_application("Firefox", "firefox.desktop", Some("firefox"));
        let entry = Entry::Application(app);

        assert_eq!(
            entry.name(),
            "Firefox",
            "Entry::name should return the app name"
        );
        assert_eq!(
            entry.icon(),
            Some("firefox"),
            "Entry::icon should return the app icon as a borrowed &str"
        );
        assert_eq!(
            entry.kind(),
            EntryKind::Application,
            "Entry::kind should return EntryKind::Application for an Application variant"
        );

        let no_icon = Entry::Application(make_application("Bare", "bare.desktop", None));
        assert_eq!(
            no_icon.icon(),
            None,
            "Entry::icon should return None when the underlying Application has no icon"
        );
    }

    #[test]
    fn entry_window_reference_round_trips() {
        let entry = Entry::Window(make_window(
            42,
            "GitHub — Pull Requests",
            Some("Firefox"),
            None,
        ));

        let reference = entry.reference();
        assert_eq!(
            reference,
            EntryRef::Window(42),
            "Entry::Window::reference() should be EntryRef::Window(id); got {reference:?}"
        );

        let entries = vec![entry.clone()];
        let resolved = resolve(&entries, &entry.reference());
        assert!(
            matches!(resolved, Some(r) if r == &entry),
            "resolve should return Some(&entry) for its own Window reference; got {resolved:?}"
        );

        // id=0 is a legal Mutter window id; must round-trip as well.
        let zero_entry = Entry::Window(make_window(0, "Zero", None, None));
        let zero_entries = vec![zero_entry.clone()];
        let zero_resolved = resolve(&zero_entries, &zero_entry.reference());
        assert!(
            matches!(zero_resolved, Some(r) if r == &zero_entry),
            "EntryRef::Window(0) should round-trip via resolve; got {zero_resolved:?}"
        );
        assert_eq!(
            zero_entry.reference(),
            EntryRef::Window(0),
            "Entry::Window with id=0 should reference EntryRef::Window(0)"
        );
    }

    #[test]
    fn resolve_finds_window_by_reference() {
        let entries = vec![
            Entry::Application(make_application("Alpha", "alpha.desktop", None)),
            Entry::Application(make_application("Beta", "beta.desktop", None)),
            Entry::Window(make_window(100, "First Window", Some("Firefox"), None)),
            Entry::Window(make_window(200, "Second Window", Some("Firefox"), None)),
            Entry::Window(make_window(300, "Third Window", Some("Thunderbird"), None)),
        ];

        let resolved = resolve(&entries, &EntryRef::Window(200));
        let found = resolved.expect("resolve should find a Window for id 200");
        assert_eq!(
            found.name(),
            "Second Window",
            "resolve should return the window whose id is 200; got {:?}",
            found.name()
        );

        // Missing Application reference returns None.
        let missing_app = EntryRef::Application("missing.desktop".into());
        assert_eq!(
            resolve(&entries, &missing_app),
            None,
            "resolve should return None for an Application desktop_id not in the slice"
        );

        // Missing Window id returns None.
        let missing_window = EntryRef::Window(999);
        assert_eq!(
            resolve(&entries, &missing_window),
            None,
            "resolve should return None for a Window id not in the slice"
        );

        // A Window ref must never resolve to an Application; sanity-check
        // with a Vec that has only Applications.
        let only_apps = vec![
            Entry::Application(make_application("Alpha", "alpha.desktop", None)),
            Entry::Application(make_application("Beta", "beta.desktop", None)),
        ];
        assert_eq!(
            resolve(&only_apps, &EntryRef::Window(123)),
            None,
            "EntryRef::Window must never resolve to an Application entry"
        );
    }

    #[test]
    fn entry_ref_window_serializes_to_tagged_json() {
        let r = EntryRef::Window(12345);

        let serialized = serde_json::to_string(&r).expect("EntryRef::Window should serialize");
        assert_eq!(
            serialized, r#"{"type":"window","id":12345}"#,
            "EntryRef::Window should serialize with tag=type/content=id and snake_case variant; got {serialized}"
        );

        let round_tripped: EntryRef =
            serde_json::from_str(&serialized).expect("EntryRef::Window should deserialize");
        assert_eq!(
            round_tripped, r,
            "EntryRef::Window should round-trip via serde_json; got {round_tripped:?}"
        );
    }

    #[test]
    fn entry_window_methods_return_window_data() {
        let entry = Entry::Window(make_window(
            7,
            "Tab Title",
            Some("Firefox"),
            Some("firefox"),
        ));

        assert_eq!(
            entry.name(),
            "Tab Title",
            "Entry::Window::name should return the window title"
        );
        assert_eq!(
            entry.icon(),
            Some("firefox"),
            "Entry::Window::icon should return the window icon as a borrowed &str"
        );
        assert_eq!(
            entry.kind(),
            EntryKind::Window,
            "Entry::Window::kind should return EntryKind::Window"
        );

        let no_icon = Entry::Window(make_window(8, "No Icon", Some("Firefox"), None));
        assert_eq!(
            no_icon.icon(),
            None,
            "Entry::Window::icon should return None when the underlying Window has no icon"
        );
    }
}
