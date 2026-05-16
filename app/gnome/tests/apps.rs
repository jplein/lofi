use lofi_gnome::{Application, gather_applications};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::tempdir;

fn write_desktop(dir: &Path, filename: &str, name: &str, exec: &str) {
    fs::create_dir_all(dir).expect("create_dir_all should succeed for test fixture dir");
    let contents = format!(
        "[Desktop Entry]\nType=Application\nName={name}\nExec={exec}\n",
        name = name,
        exec = exec,
    );
    let path = dir.join(filename);
    fs::write(&path, contents).expect("write should succeed for test fixture .desktop file");
}

#[test]
fn gather_applications_lists_all_desktop_files_in_supplied_dirs() {
    let temp = tempdir().expect("tempdir should be creatable");
    let temp_path = temp.path();

    let data_home_apps = temp_path.join("data_home").join("applications");
    let usr_share_apps = temp_path
        .join("data_dirs")
        .join("usr_share")
        .join("applications");

    write_desktop(&data_home_apps, "alpha.desktop", "Alpha", "true");
    write_desktop(&data_home_apps, "beta.desktop", "Beta", "true");
    write_desktop(&usr_share_apps, "gamma.desktop", "Gamma", "true");

    // Non-.desktop file should be ignored.
    fs::write(data_home_apps.join("readme.txt"), "not a desktop file\n")
        .expect("write should succeed for readme.txt");

    // Empty subdir should not be recursed into.
    fs::create_dir_all(data_home_apps.join("subdir"))
        .expect("create_dir_all should succeed for empty subdir");

    // Non-existent path must be silently skipped.
    let nonexistent = temp_path.join("nonexistent").join("applications");

    let dirs: Vec<PathBuf> = vec![data_home_apps.clone(), usr_share_apps.clone(), nonexistent];

    let mut apps: Vec<Application> = gather_applications(&dirs);
    apps.sort_by(|a, b| a.desktop_id.cmp(&b.desktop_id));

    let expected_app_count = 3;
    assert_eq!(
        apps.len(),
        expected_app_count,
        "expected {expected_app_count} apps, got {apps:?}"
    );

    let names: Vec<String> = apps.iter().map(|a| a.name.clone()).collect();
    assert_eq!(
        names,
        vec!["Alpha".to_string(), "Beta".to_string(), "Gamma".to_string()],
        "names sorted by desktop_id should be Alpha, Beta, Gamma; got {names:?}"
    );

    // `gio::DesktopAppInfo::id()` may return None for tempdir paths, in which case
    // the fallback yields the bare file stem. Strip any trailing `.desktop` and any
    // leading directory components before comparing.
    let stems: BTreeSet<String> = apps
        .iter()
        .map(|a| {
            let id = a.desktop_id.as_str();
            let no_ext = id.strip_suffix(".desktop").unwrap_or(id);
            let base = Path::new(no_ext)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(no_ext);
            base.to_string()
        })
        .collect();

    let expected: BTreeSet<String> = ["alpha", "beta", "gamma"]
        .iter()
        .map(|s| (*s).to_string())
        .collect();

    assert_eq!(
        stems, expected,
        "desktop_id stems should be alpha/beta/gamma; got {stems:?}"
    );
}
