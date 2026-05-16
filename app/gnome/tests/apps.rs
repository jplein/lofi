use lofi_gnome::{Application, gather_applications};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::tempdir;

fn write_desktop(dir: &Path, filename: &str, name: &str, exec: &str, icon: &str) {
    fs::create_dir_all(dir).expect("create_dir_all should succeed for test fixture dir");
    let contents = format!(
        "[Desktop Entry]\nType=Application\nName={name}\nExec={exec}\nIcon={icon}\n",
        name = name,
        exec = exec,
        icon = icon,
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

    write_desktop(
        &data_home_apps,
        "alpha.desktop",
        "Alpha",
        "true",
        "test-icon-alpha",
    );
    write_desktop(
        &data_home_apps,
        "beta.desktop",
        "Beta",
        "true",
        "test-icon-beta",
    );
    write_desktop(
        &usr_share_apps,
        "gamma.desktop",
        "Gamma",
        "true",
        "test-icon-gamma",
    );

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

    let desktop_ids: Vec<String> = apps.iter().map(|a| a.desktop_id.clone()).collect();
    assert_eq!(
        desktop_ids,
        vec![
            "alpha.desktop".to_string(),
            "beta.desktop".to_string(),
            "gamma.desktop".to_string(),
        ],
        "desktop_ids should be canonical .desktop-suffixed names sorted; got {desktop_ids:?}"
    );

    let icons: Vec<Option<String>> = apps.iter().map(|a| a.icon.clone()).collect();
    assert_eq!(
        icons,
        vec![
            Some("test-icon-alpha".to_string()),
            Some("test-icon-beta".to_string()),
            Some("test-icon-gamma".to_string()),
        ],
        "icons sorted by desktop_id should match the fixtures; got {icons:?}"
    );
}
