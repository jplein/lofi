pub mod matcher;
pub use matcher::search;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Application {
    pub name: String,
    pub desktop_id: String,
    pub icon: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EntryKind {
    Application,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    Application(Application),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", content = "id", rename_all = "snake_case")]
pub enum EntryRef {
    Application(String),
}

impl Entry {
    pub fn name(&self) -> &str {
        match self {
            Entry::Application(app) => app.name.as_str(),
        }
    }

    pub fn icon(&self) -> Option<&str> {
        match self {
            Entry::Application(app) => app.icon.as_deref(),
        }
    }

    pub fn kind(&self) -> EntryKind {
        match self {
            Entry::Application(_) => EntryKind::Application,
        }
    }

    pub fn reference(&self) -> EntryRef {
        match self {
            Entry::Application(app) => EntryRef::Application(app.desktop_id.clone()),
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
}
